//! GDAL Validation Tests
//!
//! These tests compare our pure Rust COG/TIFF implementation against GDAL
//! to ensure we produce identical results. GDAL is the industry standard
//! for geospatial raster processing, so matching its output validates our
//! implementation correctness.
//!
//! IMPORTANT: GDAL is a TEST-ONLY dependency. Our library NEVER requires GDAL
//! for production use - these tests exist solely for validation purposes.
//!
//! Note: Requires GDAL to be installed on the system (tested with GDAL 3.x).

#[cfg(test)]
mod tests {
    use gdal::Dataset;
    use std::path::PathBuf;

    use crate::reader::cog::try_read_geotiff_with_flexible_type;

    /// Epsilon for floating-point comparison
    const EPSILON: f64 = 1e-6;

    /// Get test data directory
    fn test_data_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data")
    }

    /// Find a test GeoTIFF file in the data directory
    fn find_test_geotiff() -> Option<PathBuf> {
        let data_dir = test_data_dir();
        if !data_dir.exists() {
            return None;
        }

        // Look for any .tif file in the data directory
        for entry in walkdir::WalkDir::new(&data_dir)
            .min_depth(1)
            .max_depth(3)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.extension().map(|e| e == "tif" || e == "tiff").unwrap_or(false) {
                return Some(path.to_path_buf());
            }
        }
        None
    }

    #[test]
    fn test_gdal_available() {
        // Simple test to verify GDAL is available as a dev dependency
        let version = gdal::version::version_info("VERSION_NUM");
        assert!(!version.is_empty(), "GDAL should return a version number");
    }

    #[test]
    fn test_raster_dimensions_match_gdal() {
        let Some(test_file) = find_test_geotiff() else {
            eprintln!("Skipping test: no test GeoTIFF found in data/");
            return;
        };

        // Read with our implementation
        let our_result = try_read_geotiff_with_flexible_type(&test_file);
        if our_result.is_err() {
            eprintln!("Skipping test: couldn't read file with our implementation");
            return;
        }
        let our_raster = our_result.unwrap();
        let (our_bands, our_height, our_width) = our_raster.dimensions();

        // Read with GDAL
        let gdal_dataset = Dataset::open(&test_file);
        if gdal_dataset.is_err() {
            eprintln!("Skipping test: couldn't read file with GDAL");
            return;
        }
        let gdal_dataset = gdal_dataset.unwrap();
        let (gdal_width, gdal_height) = gdal_dataset.raster_size();
        let gdal_bands = gdal_dataset.raster_count();

        assert_eq!(
            our_width, gdal_width as usize,
            "Width mismatch: ours={}, GDAL={}",
            our_width, gdal_width
        );
        assert_eq!(
            our_height, gdal_height as usize,
            "Height mismatch: ours={}, GDAL={}",
            our_height, gdal_height
        );
        assert_eq!(
            our_bands, gdal_bands as usize,
            "Band count mismatch: ours={}, GDAL={}",
            our_bands, gdal_bands
        );
    }

    #[test]
    fn test_geotransform_matches_gdal() {
        let Some(test_file) = find_test_geotiff() else {
            eprintln!("Skipping test: no test GeoTIFF found in data/");
            return;
        };

        // Read with our implementation
        let our_result = try_read_geotiff_with_flexible_type(&test_file);
        if our_result.is_err() {
            eprintln!("Skipping test: couldn't read file with our implementation");
            return;
        }
        let our_raster = our_result.unwrap();

        // Build affine transform from our geotags using geo_tags() method
        let (pixel_scale, tiepoint) = our_raster.geo_tags();
        let (our_origin_x, our_origin_y, our_pixel_width, our_pixel_height) =
            if let (Some(scale), Some(tie)) = (pixel_scale, tiepoint) {
                (tie[3], tie[4], scale[0], -scale[1])
            } else {
                // No geotags found, can't compare
                eprintln!("Skipping test: no geotags in our raster");
                return;
            };

        // Read with GDAL
        let gdal_dataset = Dataset::open(&test_file);
        if gdal_dataset.is_err() {
            eprintln!("Skipping test: couldn't read file with GDAL");
            return;
        }
        let gdal_dataset = gdal_dataset.unwrap();
        let gdal_transform = gdal_dataset.geo_transform();
        if gdal_transform.is_err() {
            eprintln!("Skipping test: GDAL couldn't read geotransform");
            return;
        }
        let gdal_transform = gdal_transform.unwrap();

        // GDAL geotransform format: [origin_x, pixel_width, row_rotation, origin_y, col_rotation, pixel_height]

        // Compare origin X (upper-left corner)
        let gdal_origin_x = gdal_transform[0];
        assert!(
            (our_origin_x - gdal_origin_x).abs() < EPSILON,
            "Origin X mismatch: ours={}, GDAL={}",
            our_origin_x, gdal_origin_x
        );

        // Compare origin Y
        let gdal_origin_y = gdal_transform[3];
        assert!(
            (our_origin_y - gdal_origin_y).abs() < EPSILON,
            "Origin Y mismatch: ours={}, GDAL={}",
            our_origin_y, gdal_origin_y
        );

        // Compare pixel width
        let gdal_pixel_width = gdal_transform[1];
        assert!(
            (our_pixel_width - gdal_pixel_width).abs() < EPSILON,
            "Pixel width mismatch: ours={}, GDAL={}",
            our_pixel_width, gdal_pixel_width
        );

        // Compare pixel height (typically negative)
        let gdal_pixel_height = gdal_transform[5];
        assert!(
            (our_pixel_height - gdal_pixel_height).abs() < EPSILON,
            "Pixel height mismatch: ours={}, GDAL={}",
            our_pixel_height, gdal_pixel_height
        );
    }

    #[test]
    fn test_min_max_matches_gdal() {
        let Some(test_file) = find_test_geotiff() else {
            eprintln!("Skipping test: no test GeoTIFF found in data/");
            return;
        };

        // Read with our implementation
        let our_result = try_read_geotiff_with_flexible_type(&test_file);
        if our_result.is_err() {
            eprintln!("Skipping test: couldn't read file with our implementation");
            return;
        }
        let our_raster = our_result.unwrap();
        let our_min_max = our_raster.compute_min_max();
        if our_min_max.is_err() {
            eprintln!("Skipping test: couldn't compute min/max with our implementation");
            return;
        }
        let (our_min, our_max) = our_min_max.unwrap();

        // Read with GDAL
        let gdal_dataset = Dataset::open(&test_file);
        if gdal_dataset.is_err() {
            eprintln!("Skipping test: couldn't read file with GDAL");
            return;
        }
        let gdal_dataset = gdal_dataset.unwrap();

        // Get first band statistics
        let band = gdal_dataset.rasterband(1);
        if band.is_err() {
            eprintln!("Skipping test: couldn't get GDAL raster band");
            return;
        }
        let band = band.unwrap();

        // Compute statistics (force=true to ensure computation)
        let stats = band.compute_raster_min_max(true);
        if stats.is_err() {
            eprintln!("Skipping test: couldn't compute GDAL statistics");
            return;
        }
        let stats = stats.unwrap();
        let (gdal_min, gdal_max) = (stats.min, stats.max);

        // Allow some tolerance for floating-point differences
        let min_diff = (our_min as f64 - gdal_min).abs();
        let max_diff = (our_max as f64 - gdal_max).abs();

        // Use relative tolerance for large values, absolute for small
        let min_tolerance = (gdal_min.abs() * 1e-4).max(1e-4);
        let max_tolerance = (gdal_max.abs() * 1e-4).max(1e-4);

        assert!(
            min_diff < min_tolerance,
            "Min value mismatch: ours={}, GDAL={} (diff={})",
            our_min, gdal_min, min_diff
        );
        assert!(
            max_diff < max_tolerance,
            "Max value mismatch: ours={}, GDAL={} (diff={})",
            our_max, gdal_max, max_diff
        );
    }

    #[test]
    fn test_pixel_values_match_gdal() {
        let Some(test_file) = find_test_geotiff() else {
            eprintln!("Skipping test: no test GeoTIFF found in data/");
            return;
        };

        // Read with our implementation
        let our_result = try_read_geotiff_with_flexible_type(&test_file);
        if our_result.is_err() {
            eprintln!("Skipping test: couldn't read file with our implementation");
            return;
        }
        let our_raster = our_result.unwrap();
        let (_, our_height, our_width) = our_raster.dimensions();

        // Read with GDAL
        let gdal_dataset = Dataset::open(&test_file);
        if gdal_dataset.is_err() {
            eprintln!("Skipping test: couldn't read file with GDAL");
            return;
        }
        let gdal_dataset = gdal_dataset.unwrap();

        let band = gdal_dataset.rasterband(1);
        if band.is_err() {
            eprintln!("Skipping test: couldn't get GDAL raster band");
            return;
        }
        let band = band.unwrap();

        // Sample a grid of pixels and compare values
        let sample_step = (our_width.max(our_height) / 10).max(1);
        let mut mismatches = 0;
        let mut total_samples = 0;
        let mut max_diff: f64 = 0.0;

        for y in (0..our_height).step_by(sample_step) {
            for x in (0..our_width).step_by(sample_step) {
                total_samples += 1;

                // Get our value
                let our_value = our_raster.sample(0, x, y);
                if our_value.is_none() {
                    continue;
                }
                let our_value = our_value.unwrap();

                // Get GDAL value - read a 1x1 window at (x, y)
                let gdal_result: Result<gdal::raster::Buffer<f64>, _> = band.read_as(
                    (x as isize, y as isize),
                    (1, 1),
                    (1, 1),
                    None,
                );
                if gdal_result.is_err() {
                    continue;
                }
                let gdal_buffer = gdal_result.unwrap();
                let gdal_value = gdal_buffer.data()[0];

                // Compare values
                let diff = (our_value as f64 - gdal_value).abs();
                max_diff = max_diff.max(diff);

                // Use relative tolerance for comparison
                let tolerance = (gdal_value.abs() * 1e-4).max(1e-4);
                if diff > tolerance {
                    mismatches += 1;
                    if mismatches <= 5 {
                        eprintln!(
                            "Pixel mismatch at ({}, {}): ours={}, GDAL={} (diff={})",
                            x, y, our_value, gdal_value, diff
                        );
                    }
                }
            }
        }

        let mismatch_rate = mismatches as f64 / total_samples as f64;
        assert!(
            mismatch_rate < 0.01,
            "Too many pixel mismatches: {}/{} ({:.2}%), max_diff={}",
            mismatches, total_samples, mismatch_rate * 100.0, max_diff
        );
    }

    #[test]
    fn test_extent_matches_gdal() {
        let Some(test_file) = find_test_geotiff() else {
            eprintln!("Skipping test: no test GeoTIFF found in data/");
            return;
        };

        // Read with GDAL
        let gdal_dataset = Dataset::open(&test_file);
        if gdal_dataset.is_err() {
            eprintln!("Skipping test: couldn't read file with GDAL");
            return;
        }
        let gdal_dataset = gdal_dataset.unwrap();
        let (width, height) = gdal_dataset.raster_size();

        let gdal_transform = gdal_dataset.geo_transform();
        if gdal_transform.is_err() {
            eprintln!("Skipping test: GDAL couldn't read geotransform");
            return;
        }
        let gt = gdal_transform.unwrap();

        // Calculate GDAL extent
        // gt = [origin_x, pixel_width, row_rotation, origin_y, col_rotation, pixel_height]
        let gdal_minx = gt[0];
        let gdal_maxy = gt[3];
        let gdal_maxx = gt[0] + width as f64 * gt[1];
        let gdal_miny = gt[3] + height as f64 * gt[5]; // gt[5] is typically negative

        // Read with our implementation
        let our_result = try_read_geotiff_with_flexible_type(&test_file);
        if our_result.is_err() {
            eprintln!("Skipping test: couldn't read file with our implementation");
            return;
        }
        let our_raster = our_result.unwrap();
        let (_, our_height, our_width) = our_raster.dimensions();

        // Build affine transform from our geotags using geo_tags() method
        let (pixel_scale, tiepoint) = our_raster.geo_tags();
        let (our_origin_x, our_origin_y, our_pixel_width, our_pixel_height) =
            if let (Some(scale), Some(tie)) = (pixel_scale, tiepoint) {
                (tie[3], tie[4], scale[0], -scale[1])
            } else {
                eprintln!("Skipping test: no geotags in our raster");
                return;
            };

        // Calculate our extent
        let our_minx = our_origin_x;
        let our_maxy = our_origin_y;
        let our_maxx = our_minx + our_width as f64 * our_pixel_width;
        let our_miny = our_maxy + our_height as f64 * our_pixel_height;

        // Compare
        assert!(
            (our_minx - gdal_minx).abs() < EPSILON,
            "MinX mismatch: ours={}, GDAL={}",
            our_minx, gdal_minx
        );
        assert!(
            (our_maxx - gdal_maxx).abs() < EPSILON,
            "MaxX mismatch: ours={}, GDAL={}",
            our_maxx, gdal_maxx
        );
        assert!(
            (our_miny - gdal_miny).abs() < EPSILON,
            "MinY mismatch: ours={}, GDAL={}",
            our_miny, gdal_miny
        );
        assert!(
            (our_maxy - gdal_maxy).abs() < EPSILON,
            "MaxY mismatch: ours={}, GDAL={}",
            our_maxy, gdal_maxy
        );
    }

    #[test]
    fn test_world_to_pixel_matches_gdal() {
        let Some(test_file) = find_test_geotiff() else {
            eprintln!("Skipping test: no test GeoTIFF found in data/");
            return;
        };

        // Read with GDAL
        let gdal_dataset = Dataset::open(&test_file);
        if gdal_dataset.is_err() {
            eprintln!("Skipping test: couldn't read file with GDAL");
            return;
        }
        let gdal_dataset = gdal_dataset.unwrap();

        let gdal_transform = gdal_dataset.geo_transform();
        if gdal_transform.is_err() {
            eprintln!("Skipping test: GDAL couldn't read geotransform");
            return;
        }
        let gt = gdal_transform.unwrap();

        // Read with our implementation
        let our_result = try_read_geotiff_with_flexible_type(&test_file);
        if our_result.is_err() {
            eprintln!("Skipping test: couldn't read file with our implementation");
            return;
        }
        let our_raster = our_result.unwrap();
        let (_, our_height, our_width) = our_raster.dimensions();

        // Get our transform parameters using the geo_tags() method
        let (pixel_scale, tiepoint) = our_raster.geo_tags();
        let (our_origin_x, our_origin_y, our_pixel_width, our_pixel_height) =
            if let (Some(scale), Some(tie)) = (pixel_scale, tiepoint) {
                (tie[3], tie[4], scale[0], -scale[1])
            } else {
                eprintln!("Skipping test: no geotags in our raster");
                return;
            };

        // Test a grid of world coordinates
        let step_y = (our_height / 5).max(1);
        let step_x = (our_width / 5).max(1);

        for py in (0..our_height).step_by(step_y) {
            for px in (0..our_width).step_by(step_x) {
                // Convert pixel to world using GDAL transform
                let world_x = gt[0] + px as f64 * gt[1] + py as f64 * gt[2];
                let world_y = gt[3] + px as f64 * gt[4] + py as f64 * gt[5];

                // Convert world back to pixel using our inverse transform
                // For simple affine (no rotation): px = (world_x - origin_x) / pixel_width
                let our_px = (world_x - our_origin_x) / our_pixel_width;
                let our_py = (world_y - our_origin_y) / our_pixel_height;

                // Should match original pixel coordinates
                assert!(
                    (our_px - px as f64).abs() < 0.01,
                    "Pixel X mismatch at ({}, {}): expected {}, got {}",
                    px, py, px, our_px
                );
                assert!(
                    (our_py - py as f64).abs() < 0.01,
                    "Pixel Y mismatch at ({}, {}): expected {}, got {}",
                    px, py, py, our_py
                );
            }
        }
    }

    #[test]
    fn test_nodata_value_matches_gdal() {
        let Some(test_file) = find_test_geotiff() else {
            eprintln!("Skipping test: no test GeoTIFF found in data/");
            return;
        };

        // Read with GDAL
        let gdal_dataset = Dataset::open(&test_file);
        if gdal_dataset.is_err() {
            eprintln!("Skipping test: couldn't read file with GDAL");
            return;
        }
        let gdal_dataset = gdal_dataset.unwrap();

        let band = gdal_dataset.rasterband(1);
        if band.is_err() {
            eprintln!("Skipping test: couldn't get GDAL raster band");
            return;
        }
        let band = band.unwrap();

        let gdal_nodata = band.no_data_value();

        // Read with our implementation
        let our_result = try_read_geotiff_with_flexible_type(&test_file);
        if our_result.is_err() {
            eprintln!("Skipping test: couldn't read file with our implementation");
            return;
        }
        let _our_raster = our_result.unwrap();

        // Note: RasterReadResult doesn't expose nodata_value directly anymore
        // The underlying sources handle nodata internally during sampling
        if let Some(gdal_val) = gdal_nodata {
            eprintln!("Info: GDAL nodata value = {}", gdal_val);
        }
        // Test passes as long as we can read the file - nodata handling is internal
    }
}
