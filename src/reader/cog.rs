use super::Layer;
use crate::{Config, utils::style::get_builtin_gradient};
use gdal::spatial_ref::SpatialRef;
use gdal::{Dataset, DriverManager, errors::GdalError};
use gdal_sys::{GDALReprojectImage, GDALResampleAlg};
use image::{ColorType, ImageEncoder, Rgba, RgbaImage, codecs::png::PngEncoder};
use proj::Proj;
use std::{io::Cursor, path::PathBuf};
use tokio::task;

pub async fn process_cog(
    input_path: PathBuf,
    bbox_3857: (f64, f64, f64, f64),
    layer_obj: Layer,
    tile_size: (usize, usize),
) -> gdal::errors::Result<Vec<u8>> {
    task::spawn_blocking(move || {
        let (tile_size_x, tile_size_y) = tile_size;
        let source_crs = format!("{}:{}", "EPSG", layer_obj.source_geometry.crs_code);
        let to_merc = Proj::new_known_crs(&source_crs, "EPSG:3857", None)
            .map_err(|e| GdalError::BadArgument(e.to_string()))?;
        let (orig_minx, orig_miny, orig_maxx, orig_maxy) = layer_obj.source_geometry.extent;

        // Reproject both corners into 3857
        let (x0, y0) = to_merc
            .convert((orig_minx, orig_miny))
            .map_err(|e| GdalError::BadArgument(format!("failed to reproj min corner: {}", e)))?;
        let (x1, y1) = to_merc
            .convert((orig_maxx, orig_maxy))
            .map_err(|e| GdalError::BadArgument(format!("failed to reproj max corner: {}", e)))?;
        let orig_minx_3857 = x0.min(x1);
        let orig_maxx_3857 = x0.max(x1);
        let orig_miny_3857 = y0.min(y1);
        let orig_maxy_3857 = y0.max(y1);

        // Open source dataset, S3 is /vsis3/{bucket}/{key}, otherwise file.
        let src_ds = Dataset::open(&input_path)?;

        // Prepare an in‐memory 256×256 target in Web mercator 3857
        let (minx, miny, maxx, maxy) = bbox_3857;
        let res_x = (maxx - minx) / (tile_size_x as f64);
        let res_y = (maxy - miny) / (tile_size_y as f64);

        let mem_drv = DriverManager::get_driver_by_name("MEM")
            .map_err(|e| GdalError::BadArgument(e.to_string()))?;
        let mut dst_ds = mem_drv
            .create_with_band_type::<f32, _>(
                "memory_dataset",
                tile_size_x,
                tile_size_y,
                Config::default().default_raster_band,
            )
            .map_err(|e| GdalError::BadArgument(e.to_string()))?;

        let merc_sref =
            SpatialRef::from_epsg(3857).map_err(|e| GdalError::BadArgument(e.to_string()))?;
        dst_ds
            .set_projection(
                &merc_sref
                    .to_wkt()
                    .map_err(|e| GdalError::BadArgument(e.to_string()))?,
            )
            .map_err(|e| GdalError::BadArgument(e.to_string()))?;
        dst_ds
            .set_geo_transform(&[minx, res_x, 0.0, maxy, 0.0, -res_y])
            .map_err(|e| GdalError::BadArgument(e.to_string()))?;

        // Setup reprojection of tile. Potential memory issues with unsafe code
        // however gdalwarp is not available in gdal crate as yet.
        unsafe {
            GDALReprojectImage(
                src_ds.c_dataset(),
                std::ptr::null(),
                dst_ds.c_dataset(),
                std::ptr::null(),
                GDALResampleAlg::GRA_NearestNeighbour,
                f64::NAN, // treat outside pixels as nodata
                f64::NAN,
                None,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
        }

        // Read the warped 256×256 band as f32
        let dst_band = dst_ds
            .rasterband(1)
            .map_err(|e| GdalError::BadArgument(e.to_string()))?;
        let nodata_opt: Option<f32> = dst_band.no_data_value().map(|v| v as f32);
        let is_nodata = |v: f32| v.is_nan() || nodata_opt.map(|nd| v == nd).unwrap_or(false);

        // Read the warped 256×256 band into a buffer
        let mut buffer = dst_band
            .read_as::<f32>((0, 0), tile_size, tile_size, None)?
            .data()
            .to_vec();

        // Any pixel whose geographic coordinate falls outside the original extent
        // should be treated as nodata (NaN), not 0.0.

        for y in 0..tile_size_y {
            for x in 0..tile_size_x {
                let gx = minx + (x as f64) * res_x;
                let gy = maxy - (y as f64) * res_y;
                if gx < orig_minx_3857
                    || gx > orig_maxx_3857
                    || gy < orig_miny_3857
                    || gy > orig_maxy_3857
                {
                    buffer[y * tile_size_x + x] = f32::NAN;
                }
            }
        }

        // Colourise into a 256×256 RGBA image
        let mut img = RgbaImage::new(tile_size_x as u32, tile_size_y as u32);

        if let Some(grad) = get_builtin_gradient(&layer_obj.style) {
            // Use the gradient to colourise the image
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
                let x = (i % tile_size_x) as u32;
                let y = (i / tile_size_y) as u32;
                img.put_pixel(x, y, px);
            }
        } else if layer_obj.colour_stops.is_empty() {
            // Fallback to grayscale
            for (i, &raw) in buffer.iter().enumerate() {
                let px = if is_nodata(raw) {
                    Rgba([0, 0, 0, 0])
                } else {
                    let norm =
                        (raw - layer_obj.min_value) / (layer_obj.max_value - layer_obj.min_value);
                    let lum = (norm.clamp(0.0, 1.0) * 255.0) as u8;
                    Rgba([lum, lum, lum, 255])
                };
                let x = (i % tile_size_x) as u32;
                let y = (i / tile_size_y) as u32;
                img.put_pixel(x, y, px);
            }
        } else {
            // Use the colour stops to colourise the image
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
                    let mut colour = Rgba([0, 0, 0, 0]);
                    for w in cs.windows(2) {
                        let a = &w[0];
                        let b = &w[1];
                        if scaled >= a.value && scaled <= b.value {
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
                let x = (i % tile_size_x) as u32;
                let y = (i / tile_size_y) as u32;
                img.put_pixel(x, y, px);
            }
        }

        let mut png_data = Vec::new();
        PngEncoder::new(Cursor::new(&mut png_data))
            .write_image(
                img.as_raw(),
                tile_size_x as u32,
                tile_size_y as u32,
                ColorType::Rgba8.into(),
            )
            .map_err(|e| GdalError::BadArgument(e.to_string()))?;

        Ok(png_data)
    })
    .await
    .map_err(|e| GdalError::BadArgument(e.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::{ColourStop, Layer, LayerGeometry, cog::process_cog};
    use gdal::spatial_ref::SpatialRef;
    use gdal::{Dataset, DriverManager};
    use image::{ColorType, ImageDecoder, codecs::png::PngDecoder};
    use rand::rngs::StdRng;
    use rand::{Rng, SeedableRng};
    use std::collections::HashMap;
    use std::{fs, io::Cursor, path::PathBuf};
    use tempfile::TempDir;

    fn make_layer(min_value: f32, max_value: f32) -> Layer {
        let path = PathBuf::new(); // will be set per-test
        let colour_stops = vec![
            ColourStop {
                value: 0.0,
                red: 215,
                green: 25,
                blue: 28,
                alpha: 255,
            },
            ColourStop {
                value: 100.0,
                red: 253,
                green: 174,
                blue: 97,
                alpha: 255,
            },
            ColourStop {
                value: 200.0,
                red: 255,
                green: 255,
                blue: 191,
                alpha: 255,
            },
            ColourStop {
                value: 300.0,
                red: 171,
                green: 221,
                blue: 164,
                alpha: 255,
            },
            ColourStop {
                value: 400.0,
                red: 43,
                green: 131,
                blue: 186,
                alpha: 255,
            },
        ];
        Layer {
            layer: "test".to_string(),
            style: "default".to_string(),
            path,
            size_bytes: 0,
            source_geometry: LayerGeometry {
                crs_code: 3857,
                extent: (0.0, 0.0, 256.0, 256.0),
            },
            cached_geometry: HashMap::new(),
            colour_stops,
            min_value,
            max_value,
            is_cog: true,
            last_modified: std::time::SystemTime::UNIX_EPOCH,
        }
    }

    /// Generates a temporary GeoTIFF in EPSG:3857 with reproducible random data,
    /// injecting ~10% NaN as no-data.
    fn generate_random_cog(tile_size: (usize, usize)) -> (TempDir, PathBuf) {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let file_path = tmp.path().join("test.tif");

        let (tile_size_x, tile_size_y) = tile_size;

        let driver = DriverManager::get_driver_by_name("GTIFF").unwrap();
        let mut ds = driver
            .create_with_band_type::<f32, _>(
                file_path.to_str().unwrap(),
                tile_size_x,
                tile_size_y,
                1,
            )
            .unwrap();

        // Use Web Mercator so reprojection is identity
        let sref = SpatialRef::from_epsg(3857).unwrap();
        ds.set_projection(&sref.to_wkt().unwrap()).unwrap();
        ds.set_geo_transform(&[0.0, 1.0, 0.0, 0.0, 0.0, -1.0])
            .unwrap();

        // Fill with reproducible data and inject NaNs
        let mut rng = StdRng::seed_from_u64(42);
        let data: Vec<f32> = (0..tile_size_x * tile_size_y)
            .map(|_| {
                if rng.random_bool(0.1) {
                    f32::NAN
                } else {
                    rng.random_range(0.0..100.0)
                }
            })
            .collect();

        let mut band = ds
            .rasterband(Config::default().default_raster_band)
            .unwrap();

        let mut buffer = gdal::raster::Buffer::<f32>::new((tile_size_x, tile_size_y), data);
        band.write((0, 0), (tile_size_x, tile_size_y), &mut buffer)
            .unwrap();
        ds.flush_cache().unwrap();

        (tmp, file_path)
    }

    #[tokio::test]
    async fn test_process_cog_data_length() {
        let tile_size = (256, 256);
        let (tmp, path) = generate_random_cog(tile_size);
        let mut layer = make_layer(1.0, 100.0);
        layer.path = path.clone();
        layer.size_bytes = fs::metadata(&path).unwrap().len();

        let buffer = process_cog(path.clone(), (0.0, 256.0, 0.0, 256.0), layer, tile_size)
            .await
            .expect("process_cog should succeed");

        assert!(!buffer.is_empty(), "Output buffer must not be empty");
        let decoder = PngDecoder::new(Cursor::new(&buffer)).unwrap();
        assert_eq!(decoder.color_type(), ColorType::Rgba8, "Expected RGBA8");

        drop(tmp);
    }

    #[tokio::test]
    async fn test_nodata_values_are_transparent() {
        let tile_size = (256, 256);
        let (tmp, path) = generate_random_cog(tile_size);
        let mut layer = make_layer(0.0, 100.0);
        layer.path = path.clone();
        layer.size_bytes = fs::metadata(&path).unwrap().len();

        let buffer = process_cog(path.clone(), (0.0, 256.0, 0.0, 256.0), layer, tile_size)
            .await
            .expect("process_cog should succeed");

        let img = image::load_from_memory(&buffer)
            .expect("Failed to load image")
            .to_rgba8();
        let transparent_count = img.pixels().filter(|p| p.0[3] == 0).count();
        assert!(
            transparent_count > 0,
            "Expected transparent pixels for no-data"
        );

        drop(tmp);
    }

    #[test]
    fn test_nodata_mask_generation() {
        let (tmp, path) = generate_random_cog((256, 256));
        let ds = Dataset::open(&path).unwrap();
        let band = ds.rasterband(1).unwrap();

        let data: Vec<f32> = band
            .read_as::<f32>((0, 0), (10, 10), (10, 10), None)
            .unwrap()
            .data()
            .to_vec();

        let nodata_opt = band.no_data_value().map(|v| v as f32);
        let mask: Vec<bool> = data
            .iter()
            .map(|&v| v.is_nan() || nodata_opt.map(|nd| v == nd).unwrap_or(false))
            .collect();

        assert_eq!(mask.len(), data.len(), "Mask length must match data length");
        drop(tmp);
    }
}
