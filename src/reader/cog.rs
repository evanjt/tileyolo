use super::{ColourStop, Layer, LayerGeometry, TileReader, TileResponse};
use crate::{Config, reader::style::get_builtin_gradient};
use gdal::{Dataset, DriverManager, errors::GdalError};
use image::{ColorType, ImageDecoder, ImageEncoder, Rgba, RgbaImage, codecs::png::PngEncoder};
use proj::Proj;
use std::io::Cursor;
use tokio::task;

async fn process_cog(
    input_path: String,
    output_path: String,
    bbox: (f64, f64, f64, f64),
    layer_obj: Layer,
) -> gdal::errors::Result<Vec<u8>> {
    task::spawn_blocking(move || {
        println!("Input path: {}", input_path);
        println!("BBox: {:?}", bbox);

        // Open COG and grab geo info
        let ds = Dataset::open(&input_path)?;
        let (raster_x, raster_y) = ds.raster_size();
        let gt = ds.geo_transform()?;
        let sref = ds.spatial_ref()?;
        let band = ds.rasterband(Config::default().default_raster_band)?;

        // Reproject bbox into raster CRS
        let (min_lon, max_lon, min_lat, max_lat) = bbox;
        let dst_epsg = sref.auth_code()?;
        let dst_srs = format!("EPSG:{}", dst_epsg);
        let transformer: Proj = Proj::new_known_crs("EPSG:4326", &dst_srs, None)
            .map_err(|e| GdalError::BadArgument(e.to_string()))?;
        let (x1, y1) = transformer
            .convert((min_lon, min_lat))
            .map_err(|e| GdalError::BadArgument(e.to_string()))?;
        let (x2, y2) = transformer
            .convert((max_lon, max_lat))
            .map_err(|e| GdalError::BadArgument(e.to_string()))?;
        let (min_x, max_x) = (x1.min(x2), x1.max(x2));
        let (min_y, max_y) = (y1.min(y2), y1.max(y2));

        // Compute pixel window
        let raw_col0 = ((min_x - gt[0]) / gt[1]).floor() as isize;
        let raw_row0 = ((max_y - gt[3]) / gt[5]).floor() as isize;
        let raw_col1 = ((max_x - gt[0]) / gt[1]).ceil() as isize;
        let raw_row1 = ((min_y - gt[3]) / gt[5]).ceil() as isize;
        let col0 = raw_col0.clamp(0, raster_x as isize);
        let row0 = raw_row0.clamp(0, raster_y as isize);
        let col1 = raw_col1.clamp(0, raster_x as isize);
        let row1 = raw_row1.clamp(0, raster_y as isize);

        let ncols = (col1 - col0).max(0) as usize;
        let nrows = (row1 - row0).max(0) as usize;
        if ncols == 0 || nrows == 0 {
            panic!("Empty window, nothing to read");
        }

        // Read the subset as f32
        let buffer: Vec<f32> = band
            .read_as::<f32>((col0, row0), (ncols, nrows), (ncols, nrows), None)?
            .data()
            .to_vec();

        // Nodata test
        let nodata_opt: Option<f32> = band.no_data_value().map(|v| v as f32);
        let is_nodata = |raw: f32| raw.is_nan() || nodata_opt.map(|nd| raw == nd).unwrap_or(false);

        // Prepare an RGBA canvas sized to our window
        let mut img: RgbaImage = RgbaImage::new(ncols as u32, nrows as u32);

        // Colourise
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
                let x = (i % ncols) as u32;
                let y = (i / ncols) as u32;
                img.put_pixel(x, y, px);
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
                let x = (i % ncols) as u32;
                let y = (i / ncols) as u32;
                img.put_pixel(x, y, px);
            }
        } else {
            // custom colour stops
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
                let x = (i % ncols) as u32;
                let y = (i / ncols) as u32;
                img.put_pixel(x, y, px);
            }
        }

        // (Optional) write out a GeoTIFF if you need it:
        let driver = DriverManager::get_driver_by_name("GTiff")?;
        let mut out_ds = driver.create_with_band_type::<f32, _>(&output_path, ncols, nrows, 1)?;
        out_ds.set_geo_transform(&[
            gt[0] + (col0 as f64) * gt[1],
            gt[1],
            gt[2],
            gt[3] + (row0 as f64) * gt[5],
            gt[4],
            gt[5],
        ])?;
        out_ds.set_spatial_ref(&sref)?;

        // Encode PNG with the correct dimensions
        let mut png_data = Vec::new();
        PngEncoder::new(Cursor::new(&mut png_data))
            .write_image(
                img.as_raw(),
                ncols as u32,
                nrows as u32,
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
    use std::fs;
    use std::path::Path;

    #[tokio::test]
    async fn test_process_cog_data_length() {
        let manifest = env!("CARGO_MANIFEST_DIR");
        let input_path = format!(
            "{}/data/default/***REMOVED***.tif",
            manifest
        );
        let output_path = format!("{}/data/test_output.tif", manifest);

        if !Path::new(&input_path).exists() {
            panic!("Test COG file not found at '{}'", input_path);
        }

        // Switzerland bbox
        let min_lon = -10.0_f64;
        let max_lon = 40.0_f64;
        let min_lat = 35.0_f64;
        let max_lat = 72.0_f64;

        // Create a dummy Layer object
        let layer = Layer {
            layer: "***REMOVED***".to_string(),
            style: "default".to_string(),
            path: Path::new(&input_path).to_path_buf(),
            size_bytes: fs::metadata(&input_path).unwrap().len(),
            geometry: LayerGeometry {
                crs_name: "EPSG".to_string(),
                crs_code: 4326,
            },
            colour_stops: vec![
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
            ],
            min_value: 1.0,
            max_value: 22613972.0,
        };

        // Process the COG file
        let buffer = process_cog(
            input_path.clone(),
            output_path.clone(),
            (min_lon, max_lon, min_lat, max_lat),
            layer,
        )
        .await
        .expect("Failed to process COG");

        // Verify the data length
        assert!(!buffer.is_empty(), "Data length should be greater than 0");

        // Check that the output is a valid PNG
        let png_decoder = image::codecs::png::PngDecoder::new(Cursor::new(&buffer))
            .expect("Failed to decode PNG");

        // Save the PNG to a file for inspection
        let output_png_path = format!("{}/data/test_output.png", manifest);
        let mut output_file =
            fs::File::create(&output_png_path).expect("Failed to create output PNG file");

        std::io::Write::write_all(&mut output_file, &buffer).expect("Failed to write PNG");

        assert_eq!(
            png_decoder.color_type(),
            ColorType::Rgba8,
            "Expected RGBA8 PNG"
        );
    }

    // #[tokio::test]
    async fn test_nodata_values_are_transparent() {
        let manifest = env!("CARGO_MANIFEST_DIR");
        let input_path = format!(
            "{}/data/default/***REMOVED***.tif",
            manifest
        );
        let output_path = format!("{}/data/nodata_test_output.tif", manifest);

        if !Path::new(&input_path).exists() {
            panic!("Test COG file not found at '{}'", input_path);
        }

        // First read the input file to identify nodata values
        let input_ds = Dataset::open(&input_path).expect("Failed to open input dataset");
        let input_band = input_ds.rasterband(1).expect("Failed to get input band");
        let nodata_value = input_band.no_data_value().map(|v| v as f32);

        println!("Nodata value in input: {:?}", nodata_value);

        // Assert that nodata values are defined in this TIFF
        assert!(
            nodata_value.is_some(),
            "Expected to find nodata values in the TIFF, but none were detected"
        );

        // Use a small bbox to ensure we have some data to test with
        let switz_min_lon = 5.9559_f64;
        let switz_max_lon = 10.4921_f64;
        let switz_min_lat = 45.8179_f64;
        let switz_max_lat = 47.8084_f64;

        // Create a dummy Layer object
        let layer = Layer {
            layer: "***REMOVED***".to_string(),
            style: "default".to_string(),
            path: Path::new(&input_path).to_path_buf(),
            size_bytes: fs::metadata(&input_path).unwrap().len(),
            geometry: LayerGeometry {
                crs_name: "EPSG".to_string(),
                crs_code: 4326,
            },
            colour_stops: vec![
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
            ],
            min_value: 0.00048219285,
            max_value: 22613972.0,
        };

        // Process the COG file
        let buffer = process_cog(
            input_path.clone(),
            output_path.clone(),
            (switz_min_lon, switz_max_lon, switz_min_lat, switz_max_lat),
            layer,
        )
        .await
        .expect("Failed to process COG");

        // Verify the buffer is not empty
        assert!(!buffer.is_empty(), "Output buffer should not be empty");

        // Decode the PNG and check for transparency
        let img = image::load_from_memory(&buffer).expect("Failed to load image from buffer");
        let rgba_img = img.to_rgba8();

        // Count transparent pixels
        let transparent_pixel_count = rgba_img
            .pixels()
            .filter(|p| p.0[3] == 0) // Check alpha channel
            .count();

        // Count total pixels
        let total_pixels = rgba_img.width() * rgba_img.height();

        println!(
            "Number of transparent pixels: {}/{} ({:.2}%)",
            transparent_pixel_count,
            total_pixels,
            (transparent_pixel_count as f32 / total_pixels as f32) * 100.0
        );

        // Check for NaN or nodata-equal values in the raw data (extra diagnosis)
        if transparent_pixel_count == 0 {
            // We need to read the raw data again to diagnose the issue
            let ds = Dataset::open(&input_path).expect("Failed to open dataset");
            let band = ds.rasterband(1).expect("Failed to get band");
            let nodata_opt = band.no_data_value().map(|v| v as f32);

            // Sample some data from the image
            let buffer_sample = band
                .read_as::<f32>((0, 0), (10, 10), (10, 10), None)
                .expect("Failed to read sample data")
                .data()
                .to_vec();

            println!("Sample data: {:?}", buffer_sample);
            println!("Nodata value: {:?}", nodata_opt);

            // Count potential nodata values in sample
            let sample_nodata_count = buffer_sample
                .iter()
                .filter(|&&v| v.is_nan() || nodata_opt.map(|nd| v == nd).unwrap_or(false))
                .count();

            println!(
                "Sample contains {} potential nodata values",
                sample_nodata_count
            );
        }

        // We expect some transparent pixels because nodata values exist
        assert!(
            transparent_pixel_count > 0,
            "No transparent pixels found in the output image. Nodata values are not being properly handled. \
             The image has {} total pixels. Check if the test area actually contains nodata values.",
            total_pixels
        );
    }
    // #[tokio::test]
    async fn test_nodata_mask_generation() {
        // 1) Locate the test file
        let manifest = env!("CARGO_MANIFEST_DIR");
        let path = Path::new(manifest)
            .join("data/default/***REMOVED***.tif");
        assert!(
            path.exists(),
            "Test GeoTIFF not found at '{}'",
            path.display()
        );

        // 2) Open the dataset and get band 1
        let ds = Dataset::open(&path).expect("Failed to open dataset");
        let band = ds.rasterband(1).expect("Failed to get raster band");

        // 3) Grab the no-data metadata (if any)
        let nodata_opt: Option<f32> = band.no_data_value().map(|v| v as f32);
        println!("no-data metadata: {:?}", nodata_opt);

        // 4) Closure to detect true no-data/null pixels
        let is_nodata = |raw: f32| raw.is_nan() || nodata_opt.map(|nd| raw == nd).unwrap_or(false);

        // 5) Read a large sample window approximating Europe's size
        let (raster_x, raster_y) = ds.raster_size();

        // Use a substantial portion of the raster for Europe-sized sample
        let sample_size = 500; // A reasonably large sample size in pixels
        let start_x = (raster_x / 4) as isize; // Start 1/4 into the image 
        let start_y = (raster_y / 4) as isize;
        let window_x = sample_size.min(raster_x as usize - start_x as usize);
        let window_y = sample_size.min(raster_y as usize - start_y as usize);

        println!(
            "Reading Europe-sized sample: {}x{} pixels from position ({},{})",
            window_x, window_y, start_x, start_y
        );

        let data: Vec<f32> = band
            .read_as::<f32>(
                (start_x, start_y),
                (window_x, window_y),
                (window_x, window_y),
                None,
            )
            .expect("Failed to read Europe-sized window")
            .data()
            .to_vec();

        // 6) Build a boolean mask and assert we found some no-data pixels
        let mask: Vec<bool> = data.iter().map(|&v| is_nodata(v)).collect();
        let count = mask.iter().filter(|&&b| b).count();
        println!(
            "Found {} no-data pixels out of {} samples ({:.2}%)",
            count,
            data.len(),
            (count as f32 / data.len() as f32) * 100.0
        );
        assert!(
            count == 0,
            "No no-data pixels (NaN or matching metadata) found in the sample window"
        );

        // 7) Now you can hand `&mask` off to any other test you need:
        //    let result = your_other_function(&mask);
        //    assert!(result, "other function failed on this mask");
    }
}
