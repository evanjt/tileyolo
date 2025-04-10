use super::{ColorStop, Layer, LayerGeometry, TileReader, TileResponse};
use crate::config::Config;
use async_trait::async_trait;
use gdal::spatial_ref::SpatialRef;
use gdal::{Dataset, DriverManager};
use image::{ColorType, ImageEncoder, Rgba, RgbaImage, codecs::png::PngEncoder};
use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;
use walkdir::WalkDir;

pub struct LocalTileReader {
    root: PathBuf,
    layers: HashMap<String, Vec<Layer>>,
}
impl LocalTileReader {
    pub fn new(root: PathBuf) -> Self {
        let mut layers: HashMap<String, Vec<Layer>> = HashMap::new();

        for entry in WalkDir::new(&root).min_depth(2).into_iter().flatten() {
            if let Some(ext) = entry.path().extension() {
                if ext == "tif" || ext == "tiff" {
                    if let Some(file_name) = entry.path().file_stem().and_then(|s| s.to_str()) {
                        if let Some(style) = entry
                            .path()
                            .parent()
                            .and_then(|p| p.file_name())
                            .and_then(|s| s.to_str())
                        {
                            let ds = match Dataset::open(entry.path()) {
                                Ok(ds) => ds,
                                Err(err) => {
                                    eprintln!("❌ Failed to open {:?}: {}", entry.path(), err);
                                    continue;
                                }
                            };

                            let sref = match ds.spatial_ref() {
                                Ok(sref) => sref,
                                Err(err) => {
                                    panic!("❌ CRS missing for '{}': {}", file_name, err);
                                }
                            };

                            let auth_name = sref.auth_name().unwrap_or_else(|| {
                                panic!("❌ Missing CRS authority for '{}'", file_name)
                            });

                            let auth_code = sref.auth_code().unwrap();
                            let style_path = entry.path().parent().unwrap().join("style.txt");
                            println!("🔍 Loading style for '{}': {:?}", file_name, style_path);

                            let color_stops = match super::style::parse_style_file(&style_path) {
                                Ok(stops) => {
                                    println!(
                                        "🎨 Parsed {} color stops for '{}'",
                                        stops.len(),
                                        file_name
                                    );
                                    stops
                                }
                                Err(err) => {
                                    eprintln!(
                                        "❌ Failed to parse style.txt for '{}': {}",
                                        file_name, err
                                    );
                                    continue;
                                }
                            };
                            let config = Config::default();
                            let band = match ds.rasterband(config.default_raster_band) {
                                Ok(band) => band,
                                Err(err) => {
                                    eprintln!(
                                        "❌ Failed to get raster band for '{}': {}",
                                        file_name, err
                                    );
                                    continue;
                                }
                            };
                            let (min_value, max_value) = match band.compute_raster_min_max(false) {
                                Ok(stats) => (stats.min as f32, stats.max as f32),
                                Err(err) => {
                                    eprintln!(
                                        "❌ Failed to get min/max for '{}': {}",
                                        file_name, err
                                    );
                                    eprintln!(
                                        "❌ Failed to get min/max for '{}': {}",
                                        file_name, err
                                    );
                                    continue;
                                }
                            };

                            let layer = Layer {
                                layer: file_name.to_string(),
                                style: style.to_string(),
                                path: entry.path().to_path_buf(),
                                size_bytes: entry.metadata().map(|m| m.len()).unwrap(),
                                geometry: LayerGeometry {
                                    crs_name: auth_name.to_string(),
                                    crs_code: auth_code,
                                },
                                color_stops,
                                min_value,
                                max_value,
                            };

                            println!(
                                "📄 Layer {:<50} | style: {:<10} | CRS: {:<5}:{:<5} | size (MB): {:>6.2} | min: {} | max: {}",
                                layer.layer,
                                layer.style,
                                auth_name,
                                auth_code,
                                layer.size_bytes as f64 / 1024.0 / 1024.0,
                                layer.min_value,
                                layer.max_value
                            );
                            layers.entry(file_name.to_string()).or_default().push(layer);
                        }
                    }
                }
            }
        }

        Self { root, layers }
    }
}

fn tile_bounds(z: u8, x: u32, y: u32) -> (f64, f64, f64, f64) {
    let tile_size = 256.0;
    let initial_resolution = 2.0 * 20037508.342789244 / tile_size;
    let res = initial_resolution / (2f64.powi(z as i32));
    let minx = x as f64 * tile_size * res - 20037508.342789244;
    let maxx = (x as f64 + 1.0) * tile_size * res - 20037508.342789244;
    let maxy = 20037508.342789244 - y as f64 * tile_size * res;
    let miny = 20037508.342789244 - (y as f64 + 1.0) * tile_size * res;
    (minx, miny, maxx, maxy)
}

#[async_trait]
impl TileReader for LocalTileReader {
    async fn list_layers(&self) -> HashMap<String, Vec<String>> {
        let mut result = HashMap::new();
        for (layer, styles) in &self.layers {
            result.insert(
                layer.clone(),
                styles.iter().map(|s| s.style.clone()).collect(),
            );
        }
        result
    }

    async fn get_tile(
        &self,
        layer: &str,
        z: u8,
        x: u32,
        y: u32,
        style: Option<&str>,
    ) -> Result<TileResponse, String> {
        let layer_obj = self
            .layers
            .get(layer)
            .and_then(|styles| {
                styles.iter().find(|s| {
                    style
                        .map(|style_name| s.style == style_name)
                        .unwrap_or(true)
                })
            })
            .ok_or_else(|| {
                format!(
                    "Tile not found for layer '{}' and style '{:?}'",
                    layer, style
                )
            })?;
        let tile_path = layer_obj.path.clone();

        let (minx, miny, maxx, maxy) = tile_bounds(z, x, y);

        let src_ds = Dataset::open(&tile_path).map_err(|e| e.to_string())?;
        let dst_srs = SpatialRef::from_epsg(3857).map_err(|e| e.to_string())?;
        let mem_driver = DriverManager::get_driver_by_name("MEM").map_err(|e| e.to_string())?;
        let mut dst_ds = mem_driver
            .create_with_band_type::<u16, _>("", 256, 256, 1)
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
        let config = Config::default();
        let band = dst_ds
            .rasterband(config.default_raster_band)
            .map_err(|e| e.to_string())?;
        let buffer = band
            .read_as::<u16>((0, 0), (256, 256), (256, 256), None)
            .map_err(|e| e.to_string())?
            .data()
            .to_vec();

        fn interpolate_color(val: f32, stops: &[ColorStop]) -> Rgba<u8> {
            for i in 0..stops.len().saturating_sub(1) {
                let cs0 = &stops[i];
                let cs1 = &stops[i + 1];
                if val >= cs0.value && val <= cs1.value {
                    let t = (val - cs0.value) / (cs1.value - cs0.value);
                    let r = ((1.0 - t) * cs0.red as f32 + t * cs1.red as f32) as u8;
                    let g = ((1.0 - t) * cs0.green as f32 + t * cs1.green as f32) as u8;
                    let b = ((1.0 - t) * cs0.blue as f32 + t * cs1.blue as f32) as u8;
                    let a = ((1.0 - t) * cs0.alpha as f32 + t * cs1.alpha as f32) as u8;
                    return Rgba([r, g, b, a]);
                }
            }

            Rgba([0, 0, 0, 0]) // fallback for out-of-range
        }

        let rgba_img = {
            let mut img = RgbaImage::new(256, 256);

            for (i, val) in buffer.iter().enumerate() {
                let raw_val = *val as f32;

                if raw_val == 0.0 {
                    img.put_pixel((i % 256) as u32, (i / 256) as u32, Rgba([0, 0, 0, 0]));
                    continue;
                }

                let norm_val =
                    (raw_val - layer_obj.min_value) / (layer_obj.max_value - layer_obj.min_value);
                let style_range_min = layer_obj
                    .color_stops
                    .first()
                    .map(|s| s.value)
                    .unwrap_or(0.0);
                let style_range_max = layer_obj.color_stops.last().map(|s| s.value).unwrap_or(1.0);
                let scaled = style_range_min + norm_val * (style_range_max - style_range_min);
                let px = interpolate_color(scaled, &layer_obj.color_stops);
                img.put_pixel((i % 256) as u32, (i / 256) as u32, px);
            }
            img
        };

        let mut png_data = Vec::new();
        let encoder = PngEncoder::new(Cursor::new(&mut png_data));
        encoder
            .write_image(rgba_img.as_raw(), 256, 256, ColorType::Rgba8.into())
            .map_err(|e| e.to_string())?;

        Ok(TileResponse {
            content_type: "image/png".to_string(),
            bytes: png_data,
        })
    }
}
