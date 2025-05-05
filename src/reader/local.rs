use crate::config::Config;
use crate::{
    reader::{
        GeometryExtent, Layer, LayerGeometry, TileReader, TileResponse,
        cog::process_cog,
        metadata::{LayerMetadata, MetadataCache, key_for, load_cache, save_cache},
    },
    utils::{status::print_layer_summary, style::is_builtin_palette},
};
use async_trait::async_trait;
use gdal::{Dataset, Metadata};
use indicatif::{ProgressBar, ProgressStyle};
use std::{
    collections::HashMap,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use walkdir::{DirEntry, WalkDir};
pub struct LocalTileReader {
    layers: HashMap<String, Vec<Layer>>,
}

impl LocalTileReader {
    pub fn new(root: &PathBuf) -> Self {
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
                    .map(|ext| {
                        ext.eq_ignore_ascii_case("tif")
                            || ext.eq_ignore_ascii_case("tiff")
                            || ext.eq_ignore_ascii_case("geotiff")
                            || ext.eq_ignore_ascii_case("geotif")
                    })
                    .unwrap_or(false)
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

        // Process each file found in the directory
        for entry in entries {
            let path = entry.path().to_path_buf();
            let file_stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("<unknown>")
                .to_string();

            // track bytes read
            let file_bytes = entry.metadata().ok().map(|m| m.len()).unwrap_or(0);
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
            if let Some(meta) = old_cache.get(&rel_key) {
                if meta.size_bytes == file_bytes && meta.last_modified == last_modified_secs {
                    let layer = meta.to_layer(&path);
                    layers.push(layer.clone());
                    new_cache.insert(rel_key.clone(), meta.clone());
                    pb.inc(1);
                    continue;
                }
            }

            // Otherwise read fresh via GDAL
            let layer = match Self::get_tiff_metadata(entry) {
                Ok(layer) => layer,
                Err(e) => {
                    pb.println(format!("❌ Failed to read file: {}", e));
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

        print_layer_summary(&layers);

        // Build a HashMap of layers keyed by layer name to allow quick access when called for
        // tiles
        let mut layers_map: HashMap<String, Vec<Layer>> = HashMap::new();
        for layer in layers {
            let layer_name = layer.layer.clone();
            layers_map.entry(layer_name).or_default().push(layer);
        }

        Self { layers: layers_map }
    }

    fn get_tiff_metadata(entry: DirEntry) -> anyhow::Result<Layer> {
        // (unchanged)
        let path = entry.path().to_path_buf();
        let ds = Dataset::open(&path)?;

        // Compute the native geotransform → extent
        let gt = ds.geo_transform()?;
        let origin_x = gt[0];
        let pixel_width = gt[1];
        let origin_y = gt[3];
        let pixel_height = gt[5];
        let (width, height) = ds.raster_size();
        let extent: GeometryExtent = (
            origin_x,
            (origin_x + pixel_width * width as f64),
            origin_y,
            (origin_y + pixel_height * height as f64),
        )
            .into();

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
        let layout_opt = ds.metadata_item("LAYOUT", "IMAGE_STRUCTURE");
        let is_cog = layout_opt
            .as_deref()
            .map(|v| v.eq_ignore_ascii_case("COG"))
            .unwrap_or(false);
        let sref = ds
            .spatial_ref()
            .unwrap_or_else(|e| panic!("❌ CRS missing for '{}': {}", file_stem, e));
        let auth_code = sref.auth_code().unwrap_or(0);
        let band = ds
            .rasterband(Config::default().default_raster_band)
            .unwrap_or_else(|e| panic!("❌ Failed to get raster band for '{}': {}", file_stem, e));
        let (min_value, max_value) = band
            .compute_raster_min_max(false)
            .map(|stats| (stats.min as f32, stats.max as f32))
            .unwrap_or_else(|e| panic!("❌ Failed to get min/max for '{}': {}", file_stem, e));
        let last_modified = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(SystemTime::now());

        Ok(Layer {
            layer: file_stem.clone(),
            style: style_name.to_string(),
            path: path.clone(),
            size_bytes: file_bytes,
            source_geometry: LayerGeometry {
                crs_code: auth_code,
                extent,
            },
            cached_geometry: HashMap::new(),
            colour_stops,
            min_value,
            max_value,
            is_cog,
            last_modified,
        })
    }
}

#[async_trait]
impl TileReader for LocalTileReader {
    async fn list_layers(&self) -> Vec<Layer> {
        let mut all_layers: Vec<Layer> = self
            .layers
            .values()
            .flat_map(|layers| layers.clone())
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
        _style: Option<&str>,
    ) -> anyhow::Result<TileResponse, String> {
        let tile_size = (256, 256);

        let layer_obj = self
            .layers
            .get(layer)
            .and_then(|styles| styles.first())
            .ok_or_else(|| format!("Layer not found: '{}'", layer))?;

        let (minx, miny, maxx, maxy) = tile_bounds_to_3857(z, x, y);

        // always hand off to process_cog; it will do the extent-check itself
        let png_data = process_cog(
            layer_obj.path.clone(),
            (minx, miny, maxx, maxy),
            layer_obj.clone(),
            tile_size,
        )
        .await
        .map_err(|e| e.to_string())?;

        Ok(TileResponse {
            content_type: "image/png".into(),
            bytes: png_data,
        })
    }
}

fn tile_bounds_to_3857(z: u8, x: u32, y: u32) -> (f64, f64, f64, f64) {
    // unchanged…
    let tile_size = 256.0;
    let initial_resolution = 2.0 * 20037508.342789244 / tile_size;
    let res = initial_resolution / (2f64.powi(z as i32));
    let minx = x as f64 * tile_size * res - 20037508.342789244;
    let maxx = (x as f64 + 1.0) * tile_size * res - 20037508.342789244;
    let maxy = 20037508.342789244 - y as f64 * tile_size * res;
    let miny = 20037508.342789244 - (y as f64 + 1.0) * tile_size * res;
    (minx, miny, maxx, maxy)
}
