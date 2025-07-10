#[cfg(test)]
pub mod test_helpers {
    use gdal::spatial_ref::SpatialRef;
    use gdal::DriverManager;
    use rand::prelude::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    /// Creates a test GeoTIFF with specified parameters
    pub fn create_test_geotiff(
        dir: &Path,
        filename: &str,
        size: (usize, usize),
        epsg: i32,
        bounds: (f64, f64, f64, f64), // (min_x, max_x, min_y, max_y)
        data_range: (f32, f32),
        nodata_fraction: f64,
        seed: u64,
    ) -> PathBuf {
        // Ensure the directory exists
        fs::create_dir_all(dir).unwrap();
        
        let file_path = dir.join(filename);
        let (width, height) = size;
        let (min_val, max_val) = data_range;

        let driver = DriverManager::get_driver_by_name("GTIFF").unwrap();
        let mut ds = driver
            .create_with_band_type::<f32, _>(
                file_path.to_str().unwrap(),
                width,
                height,
                1,
            )
            .unwrap();

        // Set projection
        let sref = SpatialRef::from_epsg(epsg as u32).unwrap();
        ds.set_projection(&sref.to_wkt().unwrap()).unwrap();

        // Calculate geo transform
        let (min_x, max_x, min_y, max_y) = bounds;
        let pixel_width = (max_x - min_x) / width as f64;
        let pixel_height = (max_y - min_y) / height as f64;
        
        // GDAL geo transform: [origin_x, pixel_width, 0, origin_y, 0, -pixel_height]
        ds.set_geo_transform(&[min_x, pixel_width, 0.0, max_y, 0.0, -pixel_height])
            .unwrap();

        // Generate data with some nodata values
        let mut rng = StdRng::seed_from_u64(seed);
        let data: Vec<f32> = (0..width * height)
            .map(|_| {
                if rng.random_bool(nodata_fraction) {
                    f32::NAN
                } else {
                    rng.random_range(min_val..max_val)
                }
            })
            .collect();

        let mut band = ds.rasterband(1).unwrap();
        band.set_no_data_value(Some(f32::NAN as f64)).ok();

        let mut buffer = gdal::raster::Buffer::<f32>::new((width, height), data);
        band.write((0, 0), (width, height), &mut buffer).unwrap();
        
        ds.flush_cache().unwrap();
        drop(ds); // Ensure file is closed
        
        file_path
    }

    /// Creates a test style file with color stops
    pub fn create_test_style_file(dir: &Path, stops: Vec<(f32, u8, u8, u8)>) -> PathBuf {
        // Ensure the directory exists
        fs::create_dir_all(dir).unwrap();
        
        let style_path = dir.join("style.txt");
        let mut content = String::new();
        
        for (value, r, g, b) in stops {
            content.push_str(&format!("{},{},{},{},255\n", value, r, g, b));
        }
        
        fs::write(&style_path, content).unwrap();
        style_path
    }

    /// Creates a complete test data directory structure
    pub fn setup_test_data_dir() -> (TempDir, PathBuf) {
        let tmp_dir = TempDir::new().unwrap();
        let data_dir = tmp_dir.path().join("data");
        fs::create_dir(&data_dir).unwrap();
        
        // Create various style directories
        let styles = vec!["viridis", "magma", "custom", "grayscale", "test-style"];
        for style in styles {
            fs::create_dir(data_dir.join(style)).unwrap();
        }
        
        (tmp_dir, data_dir)
    }

}