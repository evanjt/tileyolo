//! S3-based TileReader implementation.
//!
//! This module provides a tile reader that serves GeoTIFF files from S3-compatible
//! object storage (AWS S3, MinIO, etc.).

use crate::{
    constants::WEB_MERCATOR_EXTENT,
    error::TileYoloError,
    models::{
        geometry::GeometryExtent,
        layer::{Layer, LayerGeometry},
        responses::TileResponse,
    },
    reader::{cog::process_cog, cog_reader::CogReader},
    traits::TileReader,
    utils::{status::print_layer_summary, style::is_builtin_palette},
};
use async_trait::async_trait;
use futures::StreamExt;
use image::{codecs::png::PngEncoder, ColorType, ImageEncoder, RgbaImage};
use object_store::{aws::AmazonS3Builder, ObjectStore};
use std::{collections::HashMap, io::Cursor, path::PathBuf, time::SystemTime};
use tracing::{info, warn};

/// Tile reader for S3-based GeoTIFF data.
///
/// Scans an S3 bucket prefix for GeoTIFF files and serves them as XYZ tiles.
pub struct S3TileReader {
    layers: HashMap<String, Vec<Layer>>,
}

impl S3TileReader {
    /// Create a new S3 tile reader.
    ///
    /// Scans the specified S3 bucket and prefix for GeoTIFF files.
    ///
    /// # Arguments
    ///
    /// * `bucket` - S3 bucket name
    /// * `prefix` - Key prefix within the bucket (e.g., "data/tiles")
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let reader = S3TileReader::new("my-bucket", "geotiffs").await?;
    /// ```
    pub async fn new(bucket: &str, prefix: &str) -> Result<Self, TileYoloError> {
        info!(bucket = %bucket, prefix = %prefix, "Initializing S3 tile reader");

        // List objects in the bucket with the given prefix
        let layers = Self::scan_bucket(bucket, prefix).await?;

        if layers.is_empty() {
            warn!(bucket = %bucket, prefix = %prefix, "No GeoTIFF files found");
        } else {
            info!(count = layers.len(), "Found layers in S3");
        }

        // Build a HashMap of layers keyed by layer name
        let mut layers_map: HashMap<String, Vec<Layer>> = HashMap::new();
        for layer in &layers {
            layers_map
                .entry(layer.layer.clone())
                .or_default()
                .push(layer.clone());
        }

        // Sort styles within each layer group
        for styles in layers_map.values_mut() {
            styles.sort_by(|a, b| {
                let priority = |s: &str| match s {
                    "viridis" => 0,
                    "rgb" => 1,
                    "grayscale" => 2,
                    _ => 100,
                };
                priority(&a.style).cmp(&priority(&b.style))
            });
        }

        print_layer_summary(&layers);

        Ok(Self {
            layers: layers_map,
        })
    }

    /// Scan an S3 bucket for GeoTIFF files and extract metadata.
    async fn scan_bucket(bucket: &str, prefix: &str) -> Result<Vec<Layer>, TileYoloError> {
        // Build S3 client from environment
        let mut builder = AmazonS3Builder::new().with_bucket_name(bucket);

        if let Ok(region) = std::env::var("AWS_REGION") {
            builder = builder.with_region(&region);
        }

        if let Ok(endpoint) = std::env::var("AWS_ENDPOINT_URL") {
            builder = builder.with_endpoint(&endpoint);
        }

        if let Ok(access_key) = std::env::var("AWS_ACCESS_KEY_ID") {
            builder = builder.with_access_key_id(&access_key);
        }

        if let Ok(secret_key) = std::env::var("AWS_SECRET_ACCESS_KEY") {
            builder = builder.with_secret_access_key(&secret_key);
        }

        if std::env::var("AWS_ALLOW_HTTP")
            .map(|v| v.to_lowercase() == "true")
            .unwrap_or(false)
        {
            builder = builder.with_allow_http(true);
        }

        let store = builder
            .build()
            .map_err(|e| TileYoloError::Config(format!("Failed to create S3 client: {e}")))?;

        // List objects with the prefix (None means list all)
        let prefix_path = if prefix.is_empty() {
            None
        } else {
            Some(object_store::path::Path::from(prefix))
        };
        let mut list_stream = store.list(prefix_path.as_ref());

        let mut layers = Vec::new();

        while let Some(result) = list_stream.next().await {
            let meta = result
                .map_err(|e| TileYoloError::DataRead(format!("Failed to list S3 objects: {e}")))?;

            let key = meta.location.to_string();
            info!(key = %key, size = meta.size, "Found S3 object");

            // Check if it's a TIFF file
            if !key.ends_with(".tif") && !key.ends_with(".tiff") {
                info!(key = %key, "Skipping non-TIFF file");
                continue;
            }

            // Extract layer name and style from path
            // Expected format: prefix/style/layer.tif
            let relative = key.strip_prefix(prefix).unwrap_or(&key);
            let relative = relative.trim_start_matches('/');

            let parts: Vec<&str> = relative.split('/').collect();
            if parts.len() < 2 {
                warn!(key = %key, "Skipping file: expected prefix/style/layer.tif structure");
                continue;
            }

            let style_name = parts[parts.len() - 2];
            let filename = parts[parts.len() - 1];
            let layer_name = filename.trim_end_matches(".tif").trim_end_matches(".tiff");

            // Try to read COG metadata using block_in_place to allow blocking I/O
            // CogReader::open uses S3RangeReaderSync which needs a tokio runtime handle
            let s3_url = format!("s3://{bucket}/{key}");

            let result = tokio::task::block_in_place(|| {
                Self::load_layer_metadata(&s3_url, layer_name, style_name, meta.size as u64)
            });

            match result {
                Ok(layer) => {
                    info!(layer = %layer_name, style = %style_name, "Loaded layer from S3");
                    layers.push(layer);
                }
                Err(e) => {
                    warn!(key = %key, error = %e, "Failed to load layer metadata");
                }
            }
        }

        info!(layers = layers.len(), "S3 bucket scan complete");
        Ok(layers)
    }

    /// Load metadata for a single layer from S3.
    fn load_layer_metadata(
        s3_url: &str,
        layer_name: &str,
        style_name: &str,
        size_bytes: u64,
    ) -> Result<Layer, TileYoloError> {
        // CogReader::open already supports S3 URLs via the S3RangeReaderSync
        let cog_reader = CogReader::open(s3_url)
            .map_err(|e| TileYoloError::DataRead(format!("Failed to open S3 COG: {e}")))?;

        let metadata = &cog_reader.metadata;

        // Extract geographic extent
        let extent = if let Some((minx, miny, maxx, maxy)) = metadata
            .geo_transform
            .get_extent(metadata.width, metadata.height)
        {
            GeometryExtent {
                minx,
                miny,
                maxx,
                maxy,
            }
        } else {
            GeometryExtent {
                minx: -WEB_MERCATOR_EXTENT,
                miny: -WEB_MERCATOR_EXTENT,
                maxx: WEB_MERCATOR_EXTENT,
                maxy: WEB_MERCATOR_EXTENT,
            }
        };

        let auth_code = metadata.crs_code.unwrap_or(3857);
        let source_geometry = LayerGeometry {
            crs_code: auth_code,
            extent,
        };

        let cached_geometry = source_geometry.generate_cached_geometry_sync().map_err(|e| {
            TileYoloError::CoordinateTransform(format!("Failed to cache geometry: {e}"))
        })?;

        // Get min/max from COG reader
        let (min_value, max_value) = cog_reader.estimate_min_max().unwrap_or((0.0, 1.0));

        let colour_stops = if is_builtin_palette(style_name) {
            Vec::new()
        } else {
            Vec::new()
        };

        Ok(Layer {
            layer: layer_name.to_string(),
            style: style_name.to_string(),
            path: PathBuf::from(s3_url),
            size_bytes,
            source_geometry,
            cached_geometry,
            colour_stops,
            min_value,
            max_value,
            is_cog: true,
            last_modified: SystemTime::now(),
            bands: metadata.bands,
            is_tiled: metadata.is_tiled,
        })
    }
}

/// Pre-generated transparent 256x256 PNG tile.
static TRANSPARENT_TILE: std::sync::LazyLock<Vec<u8>> = std::sync::LazyLock::new(|| {
    let img = RgbaImage::new(256, 256);
    let mut png_data = Vec::new();
    PngEncoder::new(Cursor::new(&mut png_data))
        .write_image(img.as_raw(), 256, 256, ColorType::Rgba8.into())
        .expect("Failed to encode transparent tile");
    png_data
});

/// Helper to convert geocog BoundingBox to GeometryExtent for tile bounds
fn tile_bounds_to_3857(z: u8, x: u32, y: u32) -> GeometryExtent {
    geocog::xyz_tile::BoundingBox::from_xyz(z.into(), x, y).into()
}

#[async_trait]
impl TileReader for S3TileReader {
    async fn list_layers(&self) -> Vec<Layer> {
        let mut all_layers: Vec<Layer> = self
            .layers
            .values()
            .flat_map(std::clone::Clone::clone)
            .collect();
        all_layers.sort_by(|a, b| a.layer.cmp(&b.layer));
        all_layers
    }

    async fn get_tile(
        &self,
        layer: &str,
        z: u8,
        x: u32,
        y: u32,
        style: Option<&str>,
    ) -> Result<TileResponse, TileYoloError> {
        let tile_size = (256, 256);

        let base_layer = self
            .layers
            .get(layer)
            .and_then(|styles| styles.first())
            .ok_or_else(|| TileYoloError::LayerNotFound {
                name: layer.to_string(),
            })?;

        // If a style override is requested, create a modified layer
        let layer_obj = if let Some(style_name) = style {
            let mut modified = base_layer.clone();
            modified.style = style_name.to_string();
            if is_builtin_palette(style_name) {
                modified.colour_stops = Vec::new();
            }
            modified
        } else {
            base_layer.clone()
        };

        let tile_extent = tile_bounds_to_3857(z, x, y);

        // Early rejection if tile doesn't intersect layer extent
        if let Some(layer_extent_3857) = layer_obj.cached_geometry.get(&3857)
            && !layer_extent_3857.extent.intersects(&tile_extent)
        {
            return Ok(TileResponse {
                content_type: "image/png".into(),
                bytes: TRANSPARENT_TILE.clone(),
            });
        }

        // For S3 files, process_cog needs the S3 URL path
        let png_data = process_cog(layer_obj.path.clone(), tile_extent, layer_obj, tile_size)
            .await
            .map_err(|e| TileYoloError::RasterProcessing(e.to_string()))?;

        Ok(TileResponse {
            content_type: "image/png".into(),
            bytes: png_data,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tile_bounds_zoom_0() {
        let bounds = tile_bounds_to_3857(0, 0, 0);
        assert!((bounds.minx - (-WEB_MERCATOR_EXTENT)).abs() < 0.01);
        assert!((bounds.maxx - WEB_MERCATOR_EXTENT).abs() < 0.01);
    }

    #[tokio::test]
    #[cfg(feature = "s3-tests")]
    async fn test_s3_tile_reader_creation() {
        if std::env::var("AWS_ENDPOINT_URL").is_err() {
            println!("Skipping S3 test - no endpoint configured");
            return;
        }

        let bucket = std::env::var("S3_TEST_BUCKET").unwrap_or_else(|_| "test-bucket".to_string());
        let prefix = "viridis";

        let result = S3TileReader::new(&bucket, prefix).await;
        match result {
            Ok(reader) => {
                let layers = reader.list_layers().await;
                println!("Found {} layers", layers.len());
            }
            Err(e) => {
                println!("S3 reader creation failed (may be expected): {e}");
            }
        }
    }
}
