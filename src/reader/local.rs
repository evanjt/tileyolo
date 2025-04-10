use super::{Layer, LayerGeometry, TileReader, TileResponse};
use async_trait::async_trait;
use gdal::spatial_ref::SpatialRef;
use gdal::{Dataset, DriverManager};
use image::{ColorType, ImageEncoder, Luma, Rgba, RgbaImage, codecs::png::PngEncoder};
use std::collections::HashMap;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub struct LocalTileReader {
    root: PathBuf,
    layers: HashMap<String, Vec<Layer>>, // layer -> styles
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

                            println!(
                                "📄 Layer '{}' | style: '{}' | CRS: {}:{}",
                                file_name, style, auth_name, auth_code
                            );

                            let layer = Layer {
                                layer: file_name.to_string(),
                                style: style.to_string(),
                                path: entry.path().to_path_buf(),
                                geometry: LayerGeometry {
                                    crs_name: auth_name.to_string(),
                                    crs_code: auth_code,
                                },
                            };

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
        let tile_path = self
            .layers
            .get(layer)
            .and_then(|styles| {
                styles.iter().find(|s| {
                    style
                        .map(|style_name| s.style == style_name)
                        .unwrap_or(true)
                })
            })
            .map(|s| s.path.clone())
            .ok_or_else(|| {
                format!(
                    "Tile not found for layer '{}' and style '{:?}'",
                    layer, style
                )
            })?;

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

        let band = dst_ds.rasterband(1).map_err(|e| e.to_string())?;
        let buffer = band
            .read_as::<u16>((0, 0), (256, 256), (256, 256), None)
            .map_err(|e| e.to_string())?
            .data()
            .to_vec();

        let mut rgba_img = RgbaImage::new(256, 256);
        for (i, val) in buffer.iter().enumerate() {
            let px = if *val == 0 {
                Rgba([0, 0, 0, 0])
            } else {
                let v = (*val >> 8) as u8;
                Rgba([v, v, v, 255])
            };
            let x = (i % 256) as u32;
            let y = (i / 256) as u32;
            rgba_img.put_pixel(x, y, px);
        }

        let mut png_data = Vec::new();
        let mut encoder = PngEncoder::new(Cursor::new(&mut png_data));
        encoder
            .write_image(rgba_img.as_raw(), 256, 256, ColorType::Rgba8.into())
            .map_err(|e| e.to_string())?;

        Ok(TileResponse {
            content_type: "image/png".to_string(),
            bytes: png_data,
        })
    }
}
