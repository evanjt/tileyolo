use gdal::{Dataset, DriverManager, errors::GdalError};
// use image::{
// ColorType, ImageEncoder,
// Rgba,
// RgbaImage,
// codecs::png::PngEncoder
// };
use proj::Proj;
use std::path::Path;
use tokio::task;

#[tokio::main]
async fn main() -> gdal::errors::Result<()> {
    // Absolute paths
    let manifest = env!("CARGO_MANIFEST_DIR");
    let input_path = format!("{}/data/***REMOVED***.tif", manifest);
    // let ds = Dataset::open("/vsis3/cogbucket/cog.tif")?; // S3 bucket
    let output_path = format!("{}/data/switzerland_extract.tif", manifest);

    if !Path::new(&input_path).exists() {
        panic!("COG file not found at '{}'", input_path);
    }

    // Switzerland bbox
    // let switz_min_lon = 7.3264_f64;
    // let switz_max_lon = 7.3664_f64;
    // let switz_min_lat = 46.218_f64;
    // let switz_max_lat = 46.258_f64;

    let switz_min_lon = 5.9559_f64;
    let switz_max_lon = 10.4921_f64;
    let switz_min_lat = 45.8179_f64;
    let switz_max_lat = 47.8084_f64;

    // Call the async process_cog function
    process_cog(
        input_path.clone(),
        output_path.clone(),
        // output_srid,
        (switz_min_lon, switz_max_lon, switz_min_lat, switz_max_lat),
    )
    .await?;

    Ok(())
}

async fn process_cog(
    input_path: String,
    output_path: String,
    bbox: (f64, f64, f64, f64),
) -> gdal::errors::Result<gdal::raster::Buffer<f32>> {
    task::spawn_blocking(move || {
        println!("Input path: {}", input_path);
        println!("BBox: {:?}", bbox);
        let ds = Dataset::open(input_path)?; // Local file or S3 bucket
        let (raster_x, raster_y) = ds.raster_size();

        // Geo info
        let gt = ds.geo_transform()?;
        let sref = ds.spatial_ref()?;
        let band = ds.rasterband(1)?;
        let (min_lon, max_lon, min_lat, max_lat) = bbox;

        // Get no data or null values
        // let nodata_opt: Option<f32> = band.no_data_value().map(|v| v as f32);
        // let is_nodata = |raw: f32| raw.is_nan() || nodata_opt.map(|nd| raw == nd).unwrap_or(false);
        // let mut img: image::ImageBuffer<Rgba<u8>, Vec<u8>> = RgbaImage::new(256, 256);

        // Reprojection
        let dst_epsg = sref.auth_code()?;
        let dst_srs = format!("EPSG:{}", dst_epsg);

        let coordinate_srs = format!("EPSG:{}", 4326);
        let transformer: Proj = Proj::new_known_crs(&coordinate_srs, &dst_srs, None)
            .map_err(|e| GdalError::BadArgument(e.to_string()))?;

        println!(
            "Transforming coordinates...from {} to {}",
            coordinate_srs, dst_srs
        );
        // Get the bounding box in the target projection
        let (x1, y1) = transformer
            .convert((min_lon, min_lat))
            .map_err(|e| GdalError::BadArgument(e.to_string()))?;
        let (x2, y2) = transformer
            .convert((max_lon, max_lat))
            .map_err(|e| GdalError::BadArgument(e.to_string()))?;
        let (min_x, max_x) = (x1.min(x2), x1.max(x2));
        let (min_y, max_y) = (y1.min(y2), y1.max(y2));

        // Get the coordinates in the raster space
        let raw_col0 = ((min_x - gt[0]) / gt[1]).floor() as isize;
        let raw_row0 = ((max_y - gt[3]) / gt[5]).floor() as isize;
        let raw_col1 = ((max_x - gt[0]) / gt[1]).ceil() as isize;
        let raw_row1 = ((min_y - gt[3]) / gt[5]).ceil() as isize;

        // Clamp the coordinates to the raster size
        let col0 = raw_col0.clamp(0, raster_x as isize);
        let row0 = raw_row0.clamp(0, raster_y as isize);
        let col1 = raw_col1.clamp(0, raster_x as isize);
        let row1 = raw_row1.clamp(0, raster_y as isize);

        let ncols = (col1 - col0).max(0) as usize;
        let nrows = (row1 - row0).max(0) as usize;
        if ncols == 0 || nrows == 0 {
            panic!("Empty window, nothing to read");
        }

        // Read subset
        let buffer: gdal::raster::Buffer<f32> =
            band.read_as::<f32>((col0, row0), (ncols, nrows), (ncols, nrows), None)?;

        // Write output
        let driver = DriverManager::get_driver_by_name("GTiff")?;
        let mut out_ds = driver.create_with_band_type::<f32, _>(output_path, ncols, nrows, 1)?;
        out_ds.set_geo_transform(&[
            gt[0] + col0 as f64 * gt[1],
            gt[1],
            gt[2],
            gt[3] + row0 as f64 * gt[5],
            gt[4],
            gt[5],
        ])?;
        out_ds.set_spatial_ref(&sref)?;

        // let data_length = buffer.len();
        println!("Data length: {}", buffer.len());

        Ok(buffer)
    })
    .await
    .map_err(|e| GdalError::BadArgument(e.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use gdal::Dataset;
    use std::fs;
    use std::path::Path;

    #[tokio::test]
    async fn test_process_cog_data_length() {
        let manifest = env!("CARGO_MANIFEST_DIR");
        let input_path = format!("{}/data/***REMOVED***.tif", manifest);
        let output_path = format!("{}/data/test_output.tif", manifest);

        if !Path::new(&input_path).exists() {
            panic!("Test COG file not found at '{}'", input_path);
        }

        // Get file size
        let file_size = fs::metadata(&input_path)
            .expect("Failed to get file metadata")
            .len();

        // Switzerland bbox
        let switz_min_lon = 5.9559_f64;
        let switz_max_lon = 10.4921_f64;
        let switz_min_lat = 45.8179_f64;
        let switz_max_lat = 47.8084_f64;

        // Process the COG file
        let buffer = process_cog(
            input_path.clone(),
            output_path.clone(),
            (switz_min_lon, switz_max_lon, switz_min_lat, switz_max_lat),
        )
        .await
        .expect("Failed to process COG");

        // Verify the data length
        let data_length = buffer.len();
        assert!(data_length > 0, "Data length should be greater than 0");

        // Calculate buffer size in bytes (f32 is 4 bytes)
        let buffer_size_bytes = data_length * std::mem::size_of::<f32>();

        // Verify the size of the dataset versus the section read
        let ds = Dataset::open(input_path).expect("Failed to open dataset");
        let (raster_x, raster_y) = ds.raster_size();
        let total_pixels = raster_x * raster_y;

        let (ncols, nrows) = buffer.shape();
        let section_pixels = ncols * nrows;

        assert!(
            section_pixels <= total_pixels,
            "Section size should not exceed total dataset size"
        );
        assert!(section_pixels > 0, "Section size should be greater than 0");

        // Compare file size to buffer size
        println!(
            "Original file size: {} bytes, Buffer size: {} bytes, Ratio: {:.2}%",
            file_size,
            buffer_size_bytes,
            (buffer_size_bytes as f64 / file_size as f64) * 100.0
        );

        // The buffer size should be smaller than the file size since we're extracting a subset
        assert!(
            buffer_size_bytes < file_size as usize,
            "Buffer size ({} bytes) should be smaller than file size ({} bytes)",
            buffer_size_bytes,
            file_size
        );

        println!(
            "Total pixels: {}, Section pixels: {}, Data length: {}",
            total_pixels, section_pixels, data_length
        );
    }

    #[tokio::test]
    async fn test_nodata_values_are_transparent() {
        let manifest = env!("CARGO_MANIFEST_DIR");
        let input_path = format!("{}/data/***REMOVED***.tif", manifest);
        let output_path = format!("{}/data/nodata_test_output.tif", manifest);

        if !Path::new(&input_path).exists() {
            panic!("Test COG file not found at '{}'", input_path);
        }

        // First read the input file to identify nodata values
        let input_ds = Dataset::open(&input_path).expect("Failed to open input dataset");
        let input_band = input_ds.rasterband(1).expect("Failed to get input band");
        let nodata_value = input_band.no_data_value().map(|v| v as f32);

        println!("Nodata value in input: {:?}", nodata_value);

        // Use a small bbox to ensure we have some data to test with
        let switz_min_lon = 5.9559_f64;
        let switz_max_lon = 10.4921_f64;
        let switz_min_lat = 45.8179_f64;
        let switz_max_lat = 47.8084_f64;

        // Process the COG file
        let buffer = process_cog(
            input_path.clone(),
            output_path.clone(),
            (switz_min_lon, switz_max_lon, switz_min_lat, switz_max_lat),
        )
        .await
        .expect("Failed to process COG");

        // Check if the output file exists
        assert!(
            Path::new(&output_path).exists(),
            "Output file was not created"
        );

        // Open the output file
        let output_ds = Dataset::open(&output_path).expect("Failed to open output dataset");
        let output_band = output_ds.rasterband(1).expect("Failed to get output band");
        let output_nodata = output_band.no_data_value().map(|v| v as f32);

        println!("Nodata value in output: {:?}", output_nodata);

        // Verify nodata value is preserved
        assert_eq!(
            nodata_value, output_nodata,
            "Nodata value should be preserved"
        );

        // Count nodata values in the buffer
        let nodata_count = if let Some(nd) = nodata_value {
            buffer
                .data()
                .iter()
                .filter(|&&val| val == nd || val.is_nan())
                .count()
        } else {
            buffer.data().iter().filter(|&&val| val.is_nan()).count()
        };

        println!("Number of nodata pixels found: {}", nodata_count);

        // If we have nodata values in the source, we should have them in the output
        if nodata_value.is_some() {
            assert!(nodata_count > 0, "Should have found some nodata values");
        }

        // Read a sample of the output to verify nodata handling
        let (ncols, nrows) = buffer.shape();
        let sample_buffer = output_band
            .read_as::<f32>((0, 0), (ncols, nrows), (ncols, nrows), None)
            .expect("Failed to read output sample");

        // Check that nodata values in the source match those in the output
        let data = buffer.data();
        let sample_data = sample_buffer.data();

        let matching_count = data
            .iter()
            .zip(sample_data.iter())
            .filter(|(src, dst)| {
                let src_is_nodata = if let Some(nd) = nodata_value {
                    **src == nd || src.is_nan()
                } else {
                    src.is_nan()
                };

                let dst_is_nodata = if let Some(nd) = output_nodata {
                    **dst == nd || dst.is_nan()
                } else {
                    dst.is_nan()
                };

                src_is_nodata == dst_is_nodata
            })
            .count();

        println!("Matching nodata values: {}/{}", matching_count, data.len());
        assert_eq!(
            matching_count,
            data.len(),
            "All nodata values should be correctly preserved"
        );
    }
}
