use crate::{
    constants::WEB_MERCATOR_EXTENT,
    error::TileYoloError,
    models::{
        geometry::GeometryExtent,
        layer::{Layer, LayerGeometry},
        responses::TileResponse,
    },
    reader::{
        cog::process_cog,
        cog_reader::CogReader,
        metadata::{key_for, load_cache, save_cache, LayerMetadata, MetadataCache},
    },
    traits::TileReader,
    utils::{status::print_layer_summary, style::is_builtin_palette},
};
use async_trait::async_trait;
use indicatif::{ProgressBar, ProgressStyle};
use std::{
    collections::HashMap,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tracing::warn;
use walkdir::{DirEntry, WalkDir};
pub struct LocalTileReader {
    layers: HashMap<String, Vec<Layer>>,
}

impl LocalTileReader {
    pub async fn new(root: &PathBuf) -> Self {
        // Load cache (CSV, one line per record)
        let cache_path = root.join(".metadata_cache.csv");
        let old_cache: MetadataCache = load_cache(&cache_path);
        let mut new_cache: MetadataCache = MetadataCache::new();

        // Gather all .tif/.tiff files under root
        let entries: Vec<DirEntry> = WalkDir::new(root)
            .min_depth(2)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| {
                e.path()
                    .extension()
                    .and_then(|s| s.to_str())
                    .is_some_and(|ext| {
                        ext.eq_ignore_ascii_case("tif")
                            || ext.eq_ignore_ascii_case("tiff")
                            || ext.eq_ignore_ascii_case("geotiff")
                            || ext.eq_ignore_ascii_case("geotif")
                    })
            })
            .collect();

        // If no files found, return empty
        if entries.is_empty() {
            return Self {
                layers: HashMap::new(),
            };
        }

        let total_files = entries.len() as u64;
        let total_bytes: u64 = entries
            .iter()
            .filter_map(|e| e.metadata().ok())
            .map(|m| m.len())
            .sum();

        // Progress bar setup and style
        let pb = ProgressBar::new(total_files);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{msg}\n[{bar:40.cyan/blue}] {pos}/{len} {percent}%")
                .unwrap()
                .progress_chars("█▇▆▅▄▃▂▁  "),
        );
        let mut loaded_bytes = 0u64;
        let mut layers: Vec<Layer> = Vec::new();
        let mut skipped_files: Vec<(PathBuf, String)> = Vec::new();

        // Process each file found in the directory
        for entry in entries {
            let path = entry.path().to_path_buf();
            let file_stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("<unknown>")
                .to_string();

            // track bytes read
            let file_bytes = entry.metadata().ok().map_or(0, |m| m.len());
            loaded_bytes += file_bytes;

            let message = format!(
                "Reading {:<30}  ({:.2}/{:.2} MiB)",
                file_stem,
                loaded_bytes as f64 / 1024.0 / 1024.0,
                total_bytes as f64 / 1024.0 / 1024.0
            );
            pb.set_message(message);

            // Build cache key (filename) + mtime
            let rel_key = key_for(&path, root);
            let last_modified = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .unwrap_or(SystemTime::now());
            let last_modified_secs = last_modified
                .duration_since(UNIX_EPOCH)
                .unwrap_or(Duration::from_secs(0))
                .as_secs();

            // If unchanged (size + mtime), reuse metadata; style re‐derived from path
            if let Some(meta) = old_cache.get(&rel_key)
                && meta.size_bytes == file_bytes && meta.last_modified == last_modified_secs {
                    let layer = meta.to_layer(&path).await;
                    layers.push(layer.clone());
                    new_cache.insert(rel_key.clone(), meta.clone());
                    pb.inc(1);
                    continue;
                }

            // Otherwise read fresh via COG reader
            let layer = match Self::get_tiff_metadata(entry).await {
                Ok(layer) => layer,
                Err(e) => {
                    let err_str = e.to_string();
                    // Categorize errors with user-friendly descriptions
                    let reason = if err_str.contains("Missing tag 324") || err_str.contains("Missing tag 325") {
                        pb.println(format!("   ⚠ Skipping: {file_stem} (not tiled)"));
                        "not_tiled"
                    } else if err_str.contains("Unsupported compression") {
                        // Extract compression code if present
                        pb.println(format!("   ⚠ Skipping: {file_stem} (unsupported compression)"));
                        "unsupported_compression"
                    } else if err_str.contains("Invalid TIFF") || err_str.contains("Invalid signature") {
                        pb.println(format!("   ⚠ Skipping: {file_stem} (not a valid TIFF)"));
                        "invalid_tiff"
                    } else if err_str.contains("geotransform") || err_str.contains("tiepoint") || err_str.contains("pixel_scale") {
                        pb.println(format!("   ⚠ Skipping: {file_stem} (missing georeferencing)"));
                        "no_georef"
                    } else if err_str.contains("failed to fill whole buffer") {
                        pb.println(format!("   ⚠ Skipping: {file_stem} (file read error)"));
                        "read_error"
                    } else {
                        pb.println(format!("   ⚠ Skipping: {file_stem} ({err_str})"));
                        "other"
                    };
                    skipped_files.push((path, reason.to_string()));
                    pb.inc(1);
                    continue; // Skip this file and continue processing others
                }
            };
            layers.push(layer.clone());
            new_cache.insert(rel_key, LayerMetadata::from_layer(&layer));
            pb.inc(1);
        }

        // Finalize
        pb.finish_with_message("✅ All files loaded!");
        save_cache(&cache_path, &new_cache);

        println!(
            "\n📦 Total bytes: {:.2} MiB",
            total_bytes as f64 / 1024.0 / 1024.0
        );
        println!("📦 Total layers: {}", layers.len());

        // Print skipped files summary with helpful instructions
        if !skipped_files.is_empty() {
            // Group files by reason
            let mut by_reason: HashMap<&str, usize> = HashMap::new();
            for (_path, reason) in &skipped_files {
                *by_reason.entry(reason.as_str()).or_insert(0) += 1;
            }

            // User-friendly descriptions for each reason (kept short to fit in box)
            let reason_descriptions: HashMap<&str, &str> = [
                ("not_tiled", "Not tiled (uses strips instead of tiles)"),
                ("read_error", "File read error (may need re-export)"),
                ("invalid_tiff", "Not a valid TIFF/GeoTIFF"),
                ("unsupported_compression", "Unsupported compression format"),
                ("no_georef", "Missing georeferencing metadata"),
                ("other", "Other issue (see above)"),
            ].into_iter().collect();

            println!();
            println!("┌────────────────────────────────────────────────────────────────────┐");
            println!("│  ⚠️  {} file(s) skipped - not compatible with TileYolo              │", skipped_files.len());
            println!("├────────────────────────────────────────────────────────────────────┤");

            for (reason, count) in &by_reason {
                let desc = reason_descriptions.get(reason).unwrap_or(&"Unknown issue");
                let content = format!("  {count:>2} file(s): {desc}");
                let padding = 66usize.saturating_sub(content.len());
                println!("│{}{}│", content, " ".repeat(padding));
            }

            println!("├────────────────────────────────────────────────────────────────────┤");
            println!("│  To fix: convert to Cloud Optimized GeoTIFF (COG) format           │");
            println!("│  $ gdal_translate -of COG -co COMPRESS=DEFLATE input.tif out.tif   │");
            println!("│  $ gdalinfo -stats out.tif                                         │");
            println!("│                                                                    │");
            println!("│  For detailed analysis: $ tileyolo check <path>                    │");
            println!("└────────────────────────────────────────────────────────────────────┘");
        }

        print_layer_summary(&layers);

        // Build a HashMap of layers keyed by layer name to allow quick access when called for
        // tiles. Sort each group so the default style priority is: viridis > rgb > grayscale > others (alphabetical)
        let mut layers_map: HashMap<String, Vec<Layer>> = HashMap::new();
        for layer in layers {
            let layer_name = layer.layer.clone();
            layers_map.entry(layer_name).or_default().push(layer);
        }

        // Sort styles within each layer group for consistent default selection
        for styles in layers_map.values_mut() {
            styles.sort_by(|a, b| {
                let priority = |s: &str| match s {
                    "viridis" => 0,
                    "rgb" => 1,
                    "grayscale" => 2,
                    "plasma" => 3,
                    "magma" => 4,
                    "inferno" => 5,
                    _ => 100, // Other styles sorted alphabetically after known ones
                };
                let pa = priority(&a.style);
                let pb = priority(&b.style);
                if pa == pb {
                    a.style.cmp(&b.style)
                } else {
                    pa.cmp(&pb)
                }
            });
        }

        Self { layers: layers_map }
    }

    async fn get_tiff_metadata(entry: DirEntry) -> anyhow::Result<Layer> {
        let path = entry.path().to_path_buf();
        let file_stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("<unknown>")
            .to_string();
        let file_bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
        let style_name = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("default");
        let colour_stops = if is_builtin_palette(style_name) {
            Vec::new()
        } else {
            let style_path = entry.path().parent().unwrap().join("style.txt");
            crate::utils::style::parse_style_file(&style_path).unwrap_or_default()
        };

        // Use CogReader for efficient metadata-only reading (no full file load!)
        let path_str = path.to_string_lossy().to_string();
        let cog_reader = CogReader::open(&path_str)
            .map_err(|e| anyhow::anyhow!("Failed to open COG: {}", e))?;

        let metadata = &cog_reader.metadata;
        let width = metadata.width;
        let height = metadata.height;

        // Extract proper geographic extent from GeoTIFF tags
        let extent = if let Some((minx, miny, maxx, maxy)) = metadata.geo_transform.get_extent(width, height) {
            GeometryExtent { minx, miny, maxx, maxy }
        } else {
            // Fallback to Web Mercator world extent if no geotags
            warn!(path = %path.display(), "No geotags found, using Web Mercator world extent");
            let web_mercator_extent = WEB_MERCATOR_EXTENT;
            GeometryExtent {
                minx: -web_mercator_extent,
                miny: -web_mercator_extent,
                maxx: web_mercator_extent,
                maxy: web_mercator_extent,
            }
        };

        // Get CRS from GeoKey directory (default to Web Mercator if not found)
        let auth_code = metadata.crs_code.unwrap_or(3857);
        let is_cog = true; // We successfully opened it with CogReader
        let is_tiled = metadata.is_tiled; // Tiled = COG-optimised, stripped = not optimised
        let bands = metadata.bands;

        // Get min/max from GDAL statistics if available, otherwise estimate from sampled tiles
        let (min_value, max_value) = cog_reader
            .estimate_min_max()
            .map_err(|e| anyhow::anyhow!("Failed to compute raster min/max: {}", e))?;

        let last_modified = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(SystemTime::now());

        // Create the Layer object
        let source_geometry = LayerGeometry {
            crs_code: auth_code,
            extent,
        };
        let cached_geometry = source_geometry.generate_cached_geometry_sync()?;

        Ok(Layer {
            layer: file_stem.clone(),
            style: style_name.to_string(),
            path: path.clone(),
            size_bytes: file_bytes,
            source_geometry,
            cached_geometry,
            colour_stops,
            min_value,
            max_value,
            is_cog,
            last_modified,
            bands,
            is_tiled,
        })
    }
}

/// Pre-generated transparent 256x256 PNG tile (cached to avoid regenerating)
static TRANSPARENT_TILE: std::sync::LazyLock<Vec<u8>> = std::sync::LazyLock::new(|| {
    use image::{RgbaImage, codecs::png::PngEncoder, ColorType, ImageEncoder};
    use std::io::Cursor;

    let img = RgbaImage::new(256, 256); // All pixels default to transparent (0,0,0,0)
    let mut png_data = Vec::new();
    PngEncoder::new(Cursor::new(&mut png_data))
        .write_image(img.as_raw(), 256, 256, ColorType::Rgba8.into())
        .expect("Failed to encode transparent tile");
    png_data
});

#[async_trait]
impl TileReader for LocalTileReader {
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

        // If a style override is requested, create a modified layer with the new style
        let layer_obj = if let Some(style_name) = style {
            let mut modified = base_layer.clone();
            modified.style = style_name.to_string();
            // For builtin palettes, colour_stops should be empty (looked up at render time)
            // For custom styles, we'd need to load from a style.txt - not supported via query param
            if is_builtin_palette(style_name) {
                modified.colour_stops = Vec::new();
            }
            modified
        } else {
            base_layer.clone()
        };

        let tile_extent: GeometryExtent = geocog::xyz_tile::BoundingBox::from_xyz(z.into(), x, y).into();

        // OPTIMIZATION: Early rejection if tile doesn't intersect layer extent
        // This avoids loading COG data for tiles that are completely outside the layer
        if let Some(layer_extent_3857) = layer_obj.cached_geometry.get(&3857)
            && !layer_extent_3857.extent.intersects(&tile_extent)
        {
            return Ok(TileResponse {
                content_type: "image/png".into(),
                bytes: TRANSPARENT_TILE.clone(),
            });
        }

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

    use crate::constants::WEB_MERCATOR_EXTENT;
    const HALF_WORLD: f64 = WEB_MERCATOR_EXTENT;

    /// Helper to convert geocog BoundingBox to GeometryExtent for tile bounds
    fn tile_bounds_to_3857(z: u8, x: u32, y: u32) -> GeometryExtent {
        geocog::xyz_tile::BoundingBox::from_xyz(z.into(), x, y).into()
    }

    #[test]
    fn test_tile_bounds_zoom_0() {
        // Zoom 0 has a single tile covering the entire world
        let bounds = tile_bounds_to_3857(0, 0, 0);

        // Should cover -20037508 to +20037508 in both X and Y
        assert!((bounds.minx - (-HALF_WORLD)).abs() < 0.01);
        assert!((bounds.maxx - HALF_WORLD).abs() < 0.01);
        assert!((bounds.miny - (-HALF_WORLD)).abs() < 0.01);
        assert!((bounds.maxy - HALF_WORLD).abs() < 0.01);
    }

    #[test]
    fn test_tile_bounds_zoom_1() {
        // Zoom 1 has 2x2 = 4 tiles

        // Top-left tile (0, 0) - NW quadrant
        let tl = tile_bounds_to_3857(1, 0, 0);
        assert!((tl.minx - (-HALF_WORLD)).abs() < 0.01);
        assert!((tl.maxx - 0.0).abs() < 0.01);
        assert!((tl.miny - 0.0).abs() < 0.01);
        assert!((tl.maxy - HALF_WORLD).abs() < 0.01);

        // Top-right tile (1, 0) - NE quadrant
        let tr = tile_bounds_to_3857(1, 1, 0);
        assert!((tr.minx - 0.0).abs() < 0.01);
        assert!((tr.maxx - HALF_WORLD).abs() < 0.01);
        assert!((tr.miny - 0.0).abs() < 0.01);
        assert!((tr.maxy - HALF_WORLD).abs() < 0.01);

        // Bottom-left tile (0, 1) - SW quadrant
        let bl = tile_bounds_to_3857(1, 0, 1);
        assert!((bl.minx - (-HALF_WORLD)).abs() < 0.01);
        assert!((bl.maxx - 0.0).abs() < 0.01);
        assert!((bl.miny - (-HALF_WORLD)).abs() < 0.01);
        assert!((bl.maxy - 0.0).abs() < 0.01);

        // Bottom-right tile (1, 1) - SE quadrant
        let br = tile_bounds_to_3857(1, 1, 1);
        assert!((br.minx - 0.0).abs() < 0.01);
        assert!((br.maxx - HALF_WORLD).abs() < 0.01);
        assert!((br.miny - (-HALF_WORLD)).abs() < 0.01);
        assert!((br.maxy - 0.0).abs() < 0.01);
    }

    #[test]
    fn test_tile_bounds_tile_size() {
        // Each tile should have the same dimensions at a given zoom level
        let z = 5;
        let expected_size = 2.0 * HALF_WORLD / (2f64.powi(z as i32));

        for x in 0..4 {
            for y in 0..4 {
                let bounds = tile_bounds_to_3857(z, x, y);
                let width = bounds.maxx - bounds.minx;
                let height = bounds.maxy - bounds.miny;

                assert!(
                    (width - expected_size).abs() < 0.01,
                    "Width mismatch at ({}, {}): {} vs {}",
                    x, y, width, expected_size
                );
                assert!(
                    (height - expected_size).abs() < 0.01,
                    "Height mismatch at ({}, {}): {} vs {}",
                    x, y, height, expected_size
                );
            }
        }
    }

    #[test]
    fn test_tile_bounds_adjacent_tiles() {
        // Adjacent tiles should share edges
        let z = 4;

        let tile_a = tile_bounds_to_3857(z, 5, 5);
        let tile_right = tile_bounds_to_3857(z, 6, 5);
        let tile_below = tile_bounds_to_3857(z, 5, 6);

        // Right edge of A should equal left edge of tile_right
        assert!(
            (tile_a.maxx - tile_right.minx).abs() < 0.01,
            "Horizontal gap: {} vs {}",
            tile_a.maxx, tile_right.minx
        );

        // Bottom edge of A should equal top edge of tile_below
        assert!(
            (tile_a.miny - tile_below.maxy).abs() < 0.01,
            "Vertical gap: {} vs {}",
            tile_a.miny, tile_below.maxy
        );
    }

    #[test]
    fn test_tile_bounds_known_tile() {
        // Test a specific tile with known bounds (zoom 2, tile 2,1 covers roughly Europe/Africa)
        let bounds = tile_bounds_to_3857(2, 2, 1);

        // This tile covers X: 0 to half_world/2, Y: 0 to half_world/2
        let expected_size = HALF_WORLD / 2.0;
        assert!((bounds.minx - 0.0).abs() < 0.01);
        assert!((bounds.maxx - expected_size).abs() < 0.01);
        assert!((bounds.miny - 0.0).abs() < 0.01);
        assert!((bounds.maxy - expected_size).abs() < 0.01);
    }

    #[test]
    fn test_tile_bounds_high_zoom() {
        // Test at a high zoom level to ensure no overflow or precision issues
        let z = 18;
        let max_tile = (1u32 << z) - 1;

        // Test corner tiles
        let tl = tile_bounds_to_3857(z, 0, 0);
        assert!(tl.minx.is_finite() && tl.maxx.is_finite());
        assert!(tl.miny.is_finite() && tl.maxy.is_finite());

        let br = tile_bounds_to_3857(z, max_tile, max_tile);
        assert!(br.minx.is_finite() && br.maxx.is_finite());
        assert!(br.miny.is_finite() && br.maxy.is_finite());

        // Tile size should be very small at high zoom
        let tile_size = tl.maxx - tl.minx;
        let expected_size = 2.0 * HALF_WORLD / (2f64.powi(z as i32));
        assert!(
            (tile_size - expected_size).abs() < 0.0001,
            "Tile size at z={}: {} vs {}",
            z, tile_size, expected_size
        );
    }
}
