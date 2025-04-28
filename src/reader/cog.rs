use super::Layer;
use crate::{Config, reader::style::get_builtin_gradient};
use gdal::spatial_ref::SpatialRef;
use gdal::{Dataset, DriverManager, errors::GdalError};
use gdal_sys::{GDALReprojectImage, GDALResampleAlg};
use image::{ColorType, ImageEncoder, Rgba, RgbaImage, codecs::png::PngEncoder};
use std::{io::Cursor, path::PathBuf};
use tokio::task;

pub async fn process_cog(
    input_path: PathBuf,
    bbox_3857: (f64, f64, f64, f64),
    layer_obj: Layer,
) -> gdal::errors::Result<Vec<u8>> {
    task::spawn_blocking(move || {
        // Open source dataset, S3 is /vsis3/{bucket}/{key}, otherwise file.
        let src_ds = Dataset::open(&input_path)?;

        // Prepare an in‐memory 256×256 target in Web mercator 3857
        let (minx, miny, maxx, maxy) = bbox_3857;
        let tile_size: usize = 256;
        let res_x = (maxx - minx) / (tile_size as f64);
        let res_y = (maxy - miny) / (tile_size as f64);

        let mem_drv = DriverManager::get_driver_by_name("MEM")
            .map_err(|e| GdalError::BadArgument(e.to_string()))?;
        let mut dst_ds = mem_drv
            .create_with_band_type::<f32, _>(
                "memory_dataset",
                tile_size,
                tile_size,
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
                0.0,
                0.0,
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

        let buffer: Vec<f32> = dst_band
            .read_as::<f32>((0, 0), (tile_size, tile_size), (tile_size, tile_size), None)?
            .data()
            .to_vec();

        // Colourise into a 256×256 RGBA image
        let mut img = RgbaImage::new(tile_size as u32, tile_size as u32);

        if let Some(grad) = get_builtin_gradient(&layer_obj.style) {
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
                let x = (i % tile_size) as u32;
                let y = (i / tile_size) as u32;
                img.put_pixel(x, y, px);
            }
        } else if layer_obj.colour_stops.is_empty() {
            for (i, &raw) in buffer.iter().enumerate() {
                let px = if is_nodata(raw) {
                    Rgba([0, 0, 0, 0])
                } else {
                    let norm =
                        (raw - layer_obj.min_value) / (layer_obj.max_value - layer_obj.min_value);
                    let lum = (norm.clamp(0.0, 1.0) * 255.0) as u8;
                    Rgba([lum, lum, lum, 255])
                };
                let x = (i % tile_size) as u32;
                let y = (i / tile_size) as u32;
                img.put_pixel(x, y, px);
            }
        } else {
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
                let x = (i % tile_size) as u32;
                let y = (i / tile_size) as u32;
                img.put_pixel(x, y, px);
            }
        }

        let mut png_data = Vec::new();
        PngEncoder::new(Cursor::new(&mut png_data))
            .write_image(
                img.as_raw(),
                tile_size as u32,
                tile_size as u32,
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
            geometry: LayerGeometry {
                crs_name: "EPSG".to_string(),
                crs_code: 3857,
            },
            colour_stops,
            min_value,
            max_value,
            is_cog: true,
            last_modified: std::time::SystemTime::UNIX_EPOCH,
        }
    }

    /// Generates a temporary GeoTIFF in EPSG:3857 with reproducible random data,
    /// injecting ~10% NaN as no-data.
    fn generate_random_cog(width: usize, height: usize) -> (TempDir, PathBuf) {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let file_path = tmp.path().join("test.tif");

        let driver = DriverManager::get_driver_by_name("GTIFF").unwrap();
        let mut ds = driver
            .create_with_band_type::<f32, _>(file_path.to_str().unwrap(), width, height, 1)
            .unwrap();

        // Use Web Mercator so reprojection is identity
        let sref = SpatialRef::from_epsg(3857).unwrap();
        ds.set_projection(&sref.to_wkt().unwrap()).unwrap();
        ds.set_geo_transform(&[0.0, 1.0, 0.0, 0.0, 0.0, -1.0])
            .unwrap();

        // Fill with reproducible data and inject NaNs
        let mut rng = StdRng::seed_from_u64(42);
        let data: Vec<f32> = (0..width * height)
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

        let mut buffer = gdal::raster::Buffer::<f32>::new((width, height), data);
        band.write((0, 0), (width, height), &mut buffer).unwrap();
        ds.flush_cache().unwrap();

        (tmp, file_path)
    }

    #[tokio::test]
    async fn test_process_cog_data_length() {
        let (tmp, path) = generate_random_cog(256, 256);
        let mut layer = make_layer(1.0, 100.0);
        layer.path = path.clone();
        layer.size_bytes = fs::metadata(&path).unwrap().len();

        let buffer = process_cog(path.clone(), (0.0, 256.0, 0.0, 256.0), layer)
            .await
            .expect("process_cog should succeed");

        assert!(!buffer.is_empty(), "Output buffer must not be empty");
        let decoder = PngDecoder::new(Cursor::new(&buffer)).unwrap();
        assert_eq!(decoder.color_type(), ColorType::Rgba8, "Expected RGBA8");

        drop(tmp);
    }

    #[tokio::test]
    async fn test_nodata_values_are_transparent() {
        let (tmp, path) = generate_random_cog(256, 256);
        let mut layer = make_layer(0.0, 100.0);
        layer.path = path.clone();
        layer.size_bytes = fs::metadata(&path).unwrap().len();

        let buffer = process_cog(path.clone(), (0.0, 256.0, 0.0, 256.0), layer)
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
        let (tmp, path) = generate_random_cog(128, 128);
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
