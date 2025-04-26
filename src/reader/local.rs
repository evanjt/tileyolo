use super::style::{get_builtin_gradient, is_builtin_palette, print_style_summary};
use super::{ColourStop, Layer, LayerGeometry, TileReader, TileResponse};
use crate::config::Config;
use async_trait::async_trait;
use colorgrad::Gradient;
use gdal::spatial_ref::SpatialRef;
use gdal::{Dataset, DriverManager, Metadata};
use image::{ColorType, ImageEncoder, Rgba, RgbaImage, codecs::png::PngEncoder};
use indicatif::{ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;
use walkdir::WalkDir; // for overview_count() / overview()

pub struct LocalTileReader {
    layers: HashMap<String, Vec<Layer>>,
}

impl LocalTileReader {
    pub fn new(root: PathBuf) -> Self {
        // 1) Gather all .tif/.tiff files under root
        let entries: Vec<_> = WalkDir::new(&root)
            .min_depth(2)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| {
                e.path()
                    .extension()
                    .and_then(|s| s.to_str())
                    .map(|ext| ext.eq_ignore_ascii_case("tif") || ext.eq_ignore_ascii_case("tiff"))
                    .unwrap_or(false)
            })
            .collect();

        let total_files = entries.len() as u64;
        let total_bytes: u64 = entries
            .iter()
            .filter_map(|e| e.metadata().ok())
            .map(|m| m.len())
            .sum();

        // 2) Set up the progress bar
        let pb = ProgressBar::new(total_files);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{msg}\n[{bar:40.cyan/blue}] {pos}/{len} {percent}%")
                .unwrap()
                .progress_chars("█▇▆▅▄▃▂▁  "),
        );

        let mut loaded_bytes = 0u64;
        let mut layers: HashMap<String, Vec<Layer>> = HashMap::new();
        let mut collected_info = Vec::new();

        // 3) Process each file
        for entry in entries {
            let path = entry.path().to_path_buf();
            let file_stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("<unknown>")
                .to_string();

            // track bytes read
            let file_bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
            loaded_bytes += file_bytes;
            let message = format!(
                "Reading {:<30}  ({:.2}/{:.2} MiB)",
                file_stem,
                loaded_bytes as f64 / 1024.0 / 1024.0,
                total_bytes as f64 / 1024.0 / 1024.0
            );
            pb.set_message(message);

            // Determine style from folder name
            let style_name = path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                .unwrap_or("default");

            // Load colour stops or mark as builtin palette
            let colour_stops = if is_builtin_palette(style_name) {
                Vec::new()
            } else {
                let style_path = entry.path().parent().unwrap().join("style.txt");
                match super::style::parse_style_file(&style_path) {
                    Ok(stops) => stops,
                    Err(err) => {
                        eprintln!(
                            "⚠️ Missing or invalid style.txt for '{}': {} -> falling back to grayscale",
                            file_stem, err
                        );
                        Vec::new()
                    }
                }
            };

            // --- common per-file logic ---
            let ds = match Dataset::open(&path) {
                Ok(ds) => ds,
                Err(err) => {
                    eprintln!("❌ Failed to open {:?}: {}", path, err);
                    pb.inc(1);
                    continue;
                }
            };

            // ←== START COG DETECTION VIA IMAGE_STRUCTURE METADATA ==→

            // Pull out just the single "LAYOUT" item from the "IMAGE_STRUCTURE" domain
            let layout_opt = ds.metadata_item("LAYOUT", "IMAGE_STRUCTURE");

            // True if LAYOUT=COG (case-insensitive), false otherwise
            let is_cog = layout_opt
                .as_deref()
                .map(|v| v.eq_ignore_ascii_case("COG"))
                .unwrap_or(false);

            // Grab the entire IMAGE_STRUCTURE block for later debugging
            let image_structure_metadata =
                ds.metadata_domain("IMAGE_STRUCTURE").unwrap_or_default();

            // Count up internal overviews (pyramid levels)
            let overviews = ds
                .rasterband(1)
                .and_then(|b| {
                    let count = b.overview_count()?;
                    let mut levels = Vec::with_capacity(count.try_into().unwrap());
                    for i in 0..count {
                        let ov = b.overview(i.try_into().unwrap())?;
                        let (w, h) = ov.size();
                        levels.push((i + 1, w, h));
                    }
                    Ok(Some(levels))
                })
                .unwrap_or(None);

            // Store just what we care about
            collected_info.push((
                file_stem.clone(),
                is_cog,
                image_structure_metadata,
                overviews,
            ));

            let sref = ds
                .spatial_ref()
                .unwrap_or_else(|e| panic!("❌ CRS missing for '{}': {}", file_stem, e));
            let auth_name = sref.auth_name().unwrap_or("UNKNOWN".to_string());
            let auth_code = sref.auth_code().unwrap_or(0);

            let band = ds
                .rasterband(Config::default().default_raster_band)
                .unwrap_or_else(|e| {
                    panic!("❌ Failed to get raster band for '{}': {}", file_stem, e)
                });

            let (min_value, max_value) = band
                .compute_raster_min_max(false)
                .map(|stats| (stats.min as f32, stats.max as f32))
                .unwrap_or_else(|e| panic!("❌ Failed to get min/max for '{}': {}", file_stem, e));

            let layer = Layer {
                layer: file_stem.clone(),
                style: style_name.to_string(),
                path: path.clone(),
                size_bytes: file_bytes,
                geometry: LayerGeometry {
                    crs_name: auth_name.to_string(),
                    crs_code: auth_code,
                },
                colour_stops,
                min_value,
                max_value,
            };

            layers.entry(layer.layer.clone()).or_default().push(layer);
            // --- end per-file ---

            pb.inc(1);
        }
        // 4) Print collected info;

        pb.finish_with_message("✅ All files loaded!");
        println!(
            "📦 Total bytes: {:.2} MiB",
            total_bytes as f64 / 1024.0 / 1024.0
        );
        println!("📦 Total layers: {}", layers.len());
        println!("📦 Total styles: {}", collected_info.len());
        for (layer_name, is_cog, image_structure_metadata, overviews) in &collected_info {
            println!(
                "Layer: {}, COG: {}, Overviews: {:?}, Metadata: {:?}",
                layer_name, is_cog, overviews, image_structure_metadata
            );
        }

        // === build style_info ===
        let mut style_info: HashMap<String, (usize, Vec<ColourStop>, f32, f32)> = HashMap::new();
        for layer_list in layers.values() {
            for layer in layer_list {
                let entry = style_info.entry(layer.style.clone()).or_insert((
                    0,
                    layer.colour_stops.clone(),
                    layer.min_value,
                    layer.max_value,
                ));
                entry.0 += 1;
                entry.1 = layer.colour_stops.clone();
                entry.2 = entry.2.min(layer.min_value);
                entry.3 = entry.3.max(layer.max_value);
            }
        }
        print_style_summary(&style_info);

        Self { layers }
    }
}

#[async_trait]
impl TileReader for LocalTileReader {
    async fn list_layers(&self) -> HashMap<String, Vec<String>> {
        let mut result = HashMap::new();
        for (layer, styles) in &self.layers {
            let style_names = styles.iter().map(|s| s.style.clone()).collect();
            result.insert(layer.clone(), style_names);
        }
        result
    }
    async fn get_tile(
        &self,
        layer: &str,
        z: u8,
        x: u32,
        y: u32,
        _style: Option<&str>,
    ) -> Result<TileResponse, String> {
        let layer_obj = self
            .layers
            .get(layer)
            .and_then(|styles| styles.first())
            .ok_or_else(|| format!("Layer not found: '{}'", layer))?;
        let tile_path = &layer_obj.path;

        // reproject into MEM as f32 so we preserve negative values
        let (minx, miny, maxx, maxy) = tile_bounds(z, x, y);
        let src_ds = Dataset::open(tile_path).map_err(|e| e.to_string())?;
        let dst_srs = SpatialRef::from_epsg(3857).map_err(|e| e.to_string())?;
        let mem_driver = DriverManager::get_driver_by_name("MEM").map_err(|e| e.to_string())?;
        let mut dst_ds = mem_driver
            .create_with_band_type::<f32, _>("", 256, 256, 1)
            .map_err(|e| e.to_string())?;
        dst_ds
            .set_projection(&dst_srs.to_wkt().map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        dst_ds
            .set_geo_transform(&[
                minx,
                (maxx - minx) / 256.0,
                0.0,
                maxy,
                0.0,
                (miny - maxy) / 256.0,
            ])
            .map_err(|e| e.to_string())?;

        unsafe {
            gdal_sys::GDALReprojectImage(
                src_ds.c_dataset(),
                std::ptr::null(),
                dst_ds.c_dataset(),
                std::ptr::null(),
                gdal_sys::GDALResampleAlg::GRA_NearestNeighbour,
                0.0,
                0.0,
                None,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };

        // pull out the band (f32) and its no-data value, if any
        let band = dst_ds
            .rasterband(Config::default().default_raster_band)
            .map_err(|e| e.to_string())?;
        let nodata_opt: Option<f32> = band.no_data_value().map(|v| v as f32);

        // read as f32 so negatives and zeros are preserved
        let buffer = band
            .read_as::<f32>((0, 0), (256, 256), (256, 256), None)
            .map_err(|e| e.to_string())?
            .data()
            .to_vec();

        let mut img = RgbaImage::new(256, 256);

        // helper to detect true no-data/null
        let is_nodata = |raw: f32| raw.is_nan() || nodata_opt.map(|nd| raw == nd).unwrap_or(false);

        if let Some(grad) = get_builtin_gradient(&layer_obj.style) {
            // built-in palette
            for (i, &raw) in buffer.iter().enumerate() {
                let px = if is_nodata(raw) {
                    Rgba([0, 0, 0, 0])
                } else {
                    let t = ((raw - layer_obj.min_value)
                        / (layer_obj.max_value - layer_obj.min_value))
                        .clamp(0.0, 1.0);
                    let [r, g, b, a] = grad.at(t).to_rgba8();
                    Rgba([r, g, b, a])
                };
                img.put_pixel((i % 256) as u32, (i / 256) as u32, px);
            }
        } else if layer_obj.colour_stops.is_empty() {
            // grayscale fallback
            for (i, &raw) in buffer.iter().enumerate() {
                let px = if is_nodata(raw) {
                    Rgba([0, 0, 0, 0])
                } else {
                    let norm =
                        (raw - layer_obj.min_value) / (layer_obj.max_value - layer_obj.min_value);
                    let lum = (norm.clamp(0.0, 1.0) * 255.0) as u8;
                    Rgba([lum, lum, lum, 255])
                };
                img.put_pixel((i % 256) as u32, (i / 256) as u32, px);
            }
        } else {
            // custom stops
            let cs = &layer_obj.colour_stops;
            let style_min = cs.first().unwrap().value;
            let style_max = cs.last().unwrap().value;
            for (i, &raw) in buffer.iter().enumerate() {
                let px = if is_nodata(raw) {
                    Rgba([0, 0, 0, 0])
                } else {
                    let norm =
                        (raw - layer_obj.min_value) / (layer_obj.max_value - layer_obj.min_value);
                    let scaled = style_min + norm.clamp(0.0, 1.0) * (style_max - style_min);
                    // find which segment we're in
                    let mut colour = Rgba([0, 0, 0, 0]);
                    for w in cs.windows(2) {
                        let a = &w[0];
                        let b = &w[1];
                        if (scaled >= a.value) && (scaled <= b.value) {
                            let t = (scaled - a.value) / (b.value - a.value);
                            let r = ((1.0 - t) * a.red as f32 + t * b.red as f32) as u8;
                            let g = ((1.0 - t) * a.green as f32 + t * b.green as f32) as u8;
                            let b_ = ((1.0 - t) * a.blue as f32 + t * b.blue as f32) as u8;
                            let a_ = ((1.0 - t) * a.alpha as f32 + t * b.alpha as f32) as u8;
                            colour = Rgba([r, g, b_, a_]);
                            break;
                        }
                    }
                    colour
                };
                img.put_pixel((i % 256) as u32, (i / 256) as u32, px);
            }
        }

        let mut png_data = Vec::new();
        PngEncoder::new(Cursor::new(&mut png_data))
            .write_image(img.as_raw(), 256, 256, ColorType::Rgba8.into())
            .map_err(|e| e.to_string())?;

        Ok(TileResponse {
            content_type: "image/png".into(),
            bytes: png_data,
        })
    }
}

fn tile_bounds(z: u8, x: u32, y: u32) -> (f64, f64, f64, f64) {
    // Function for converting Web Mercator "Slippy map" tile coordinates
    // to bounding box
    // https://wiki.openstreetmap.org/wiki/Slippy_map_tilenames

    let tile_size = 256.0;
    let initial_resolution = 2.0 * 20037508.342789244 / tile_size;
    let res = initial_resolution / (2f64.powi(z as i32));
    let minx = x as f64 * tile_size * res - 20037508.342789244;
    let maxx = (x as f64 + 1.0) * tile_size * res - 20037508.342789244;
    let maxy = 20037508.342789244 - y as f64 * tile_size * res;
    let miny = 20037508.342789244 - (y as f64 + 1.0) * tile_size * res;
    (minx, miny, maxx, maxy)
}
