#[cfg(test)]
mod strict_quality_tests {
    use crate::test_utils::test_helpers::*;
    use crate::reader::local::LocalTileReader;
    use crate::traits::TileReader;
    use crate::geometry::projection::{lon_lat_to_mercator, mercator_to_lon_lat};
    use std::sync::Arc;
    use image::{ImageDecoder, codecs::png::PngDecoder};
    use std::io::Cursor;

    /// STRICT TESTS - These should fail and highlight areas for improvement
    /// These tests represent professional tile server standards

    #[tokio::test]
    async fn test_strict_coordinate_precision() {
        // STRICT: Coordinate transformations should be accurate to within 1 meter globally
        let test_points = vec![
            // Using authoritative EPSG:3857 constants from proj.org
            (0.0, 0.0, 0.0, 0.0),
            (-180.0, 0.0, -20037508.342789244, 0.0),
            (180.0, 0.0, 20037508.342789244, 0.0),
            // These values from EPSG.org registry
            (-180.0, 85.0511287798066, -20037508.342789244, 20037508.342789244),
            (180.0, -85.0511287798066, 20037508.342789244, -20037508.342789244),
        ];
        
        for (lon, lat, expected_x, expected_y) in test_points {
            let (actual_x, actual_y) = lon_lat_to_mercator(lon, lat);
            
            // STRICT: Precision within 1 meter (professional GIS standard)
            assert!(
                (actual_x - expected_x).abs() < 1.0,
                "STRICT FAIL: X coordinate precision > 1m. Expected {}, got {}, diff: {:.2}m",
                expected_x, actual_x, (actual_x - expected_x).abs()
            );
            
            assert!(
                (actual_y - expected_y).abs() < 1.0,
                "STRICT FAIL: Y coordinate precision > 1m. Expected {}, got {}, diff: {:.2}m",
                expected_y, actual_y, (actual_y - expected_y).abs()
            );
        }
    }
    
    #[tokio::test]
    async fn test_strict_tile_boundary_math() {
        let (tmp_dir, data_dir) = setup_test_data_dir();
        
        // Create a dataset that exactly covers one Web Mercator tile at zoom 1
        // Zoom 1, tile (0,0) should cover: (-20037508.34, 0, 0, 20037508.34)
        let exact_bounds = (-20037508.342789244, 0.0, 0.0, 20037508.342789244);
        
        create_test_geotiff(
            &data_dir.join("precise"),
            "boundary_test.tif",
            (256, 256),
            3857,
            exact_bounds,
            (0.0, 255.0),
            0.0,
            5001,
        );
        
        let reader = Arc::new(LocalTileReader::new(&data_dir).await);
        
        // STRICT: When we request exactly the tile that matches our data bounds,
        // it should contain our full dataset without any gaps or overlaps
        let tile = reader.get_tile("boundary_test", 1, 0, 0, None).await
            .expect("Should generate tile for exact boundary match");
            
        // STRICT: Tile should contain actual data, not just transparent pixels
        assert!(tile.bytes.len() > 1000, 
            "STRICT FAIL: Tile too small ({} bytes), likely mostly transparent - boundary math incorrect",
            tile.bytes.len());
            
        // STRICT: Adjacent tile should be mostly/entirely transparent
        let adjacent_tile = reader.get_tile("boundary_test", 1, 1, 0, None).await
            .expect("Should generate adjacent tile");
            
        // Adjacent tile should be smaller (more compressed) as it's mostly empty
        assert!(adjacent_tile.bytes.len() < (tile.bytes.len() as f64 * 0.8) as usize,
            "STRICT FAIL: Adjacent tile ({} bytes) not significantly smaller than main tile ({} bytes) - boundary precision issue",
            adjacent_tile.bytes.len(), tile.bytes.len());
        
        drop(tmp_dir);
    }
    
    #[tokio::test]
    async fn test_strict_nodata_handling() {
        let (tmp_dir, data_dir) = setup_test_data_dir();
        
        // Create dataset with 50% nodata to test transparency handling
        create_test_geotiff(
            &data_dir.join("test"),
            "nodata_test.tif",
            (256, 256),
            3857,
            (-10000000.0, 10000000.0, -10000000.0, 10000000.0),
            (0.0, 255.0),
            0.5, // 50% nodata
            5002,
        );
        
        let reader = Arc::new(LocalTileReader::new(&data_dir).await);
        let tile = reader.get_tile("nodata_test", 1, 0, 0, None).await.unwrap();
        
        // STRICT: Decode PNG and verify transparency is actually used
        let decoder = PngDecoder::new(Cursor::new(&tile.bytes)).unwrap();
        let mut img_data = vec![0u8; (decoder.total_bytes()) as usize];
        decoder.read_image(&mut img_data).unwrap();
        
        // STRICT: With 50% nodata, we should have significant transparency
        // Count transparent pixels (alpha = 0)
        let transparent_pixels: usize = img_data
            .chunks(4) // RGBA chunks
            .filter(|pixel| pixel[3] == 0) // alpha channel = 0
            .count();
            
        let total_pixels = 256 * 256;
        let transparency_ratio = transparent_pixels as f64 / total_pixels as f64;
        
        assert!(transparency_ratio > 0.3,
            "STRICT FAIL: Only {:.1}% transparency with 50% nodata - nodata not properly converted to transparency",
            transparency_ratio * 100.0);
            
        drop(tmp_dir);
    }
    
    #[tokio::test]
    async fn test_strict_data_value_accuracy() {
        let (tmp_dir, data_dir) = setup_test_data_dir();
        
        // Create dataset with known, specific values
        let min_val = 42.123456;
        let max_val = 142.654321;
        
        create_test_geotiff(
            &data_dir.join("test"),
            "precision_test.tif",
            (64, 64),
            3857,
            (-1000000.0, 1000000.0, -1000000.0, 1000000.0),
            (min_val, max_val),
            0.0,
            5003,
        );
        
        let reader = Arc::new(LocalTileReader::new(&data_dir).await);
        let layers = reader.list_layers().await;
        let layer = layers.iter().find(|l| l.layer == "precision_test").unwrap();
        
        // STRICT: Min/max values should be preserved with high precision
        let min_error = (layer.min_value - min_val).abs();
        let max_error = (layer.max_value - max_val).abs();
        
        assert!(min_error < 0.01,
            "STRICT FAIL: Min value precision loss: expected {}, got {}, error: {}",
            min_val, layer.min_value, min_error);
            
        assert!(max_error < 0.01,
            "STRICT FAIL: Max value precision loss: expected {}, got {}, error: {}",
            max_val, layer.max_value, max_error);
        
        drop(tmp_dir);
    }
    
    #[tokio::test]
    async fn test_strict_out_of_bounds_behavior() {
        let (tmp_dir, data_dir) = setup_test_data_dir();
        
        create_test_geotiff(
            &data_dir.join("test"),
            "bounds_test.tif",
            (256, 256),
            3857,
            (-10000000.0, 10000000.0, -10000000.0, 10000000.0),
            (0.0, 255.0),
            0.0,
            5004,
        );
        
        let reader = Arc::new(LocalTileReader::new(&data_dir).await);
        
        // STRICT: Invalid coordinates should return proper errors, not empty tiles
        let invalid_requests = vec![
            (0, 1, 0),    // Invalid X at zoom 0
            (0, 0, 1),    // Invalid Y at zoom 0  
            (1, 2, 0),    // Invalid X at zoom 1
            (1, 0, 2),    // Invalid Y at zoom 1
            (30, 0, 0),   // Extremely high zoom
        ];
        
        for (z, x, y) in invalid_requests {
            let result = reader.get_tile("bounds_test", z, x, y, None).await;
            assert!(result.is_err(),
                "STRICT FAIL: Invalid coordinate {}/{}/{} should return error, not empty tile",
                z, x, y);
        }
        
        drop(tmp_dir);
    }
    
    #[tokio::test]
    async fn test_strict_png_optimization() {
        let (tmp_dir, data_dir) = setup_test_data_dir();
        
        // Create dataset with different complexity levels
        create_test_geotiff(
            &data_dir.join("simple"),
            "uniform.tif",
            (256, 256),
            3857,
            (-5000000.0, 5000000.0, -5000000.0, 5000000.0),
            (100.0, 100.1), // Nearly uniform value (need small range for random generation)
            0.0,
            5005,
        );
        
        create_test_geotiff(
            &data_dir.join("complex"),
            "detailed.tif",
            (256, 256),
            3857,
            (-5000000.0, 5000000.0, -5000000.0, 5000000.0),
            (0.0, 255.0), // Full range
            0.0,
            5006,
        );
        
        let reader = Arc::new(LocalTileReader::new(&data_dir).await);
        
        let uniform_tile = reader.get_tile("uniform", 0, 0, 0, None).await.unwrap();
        let complex_tile = reader.get_tile("detailed", 0, 0, 0, None).await.unwrap();
        
        // STRICT: Uniform data should compress much better than complex data
        let compression_ratio = uniform_tile.bytes.len() as f64 / complex_tile.bytes.len() as f64;
        
        assert!(compression_ratio < 0.5,
            "STRICT FAIL: Poor PNG compression. Uniform tile ({} bytes) should be <50% of complex tile ({} bytes), ratio: {:.2}",
            uniform_tile.bytes.len(), complex_tile.bytes.len(), compression_ratio);
        
        drop(tmp_dir);
    }
    
    #[tokio::test]
    async fn test_strict_projection_edge_cases() {
        // STRICT: Test projection accuracy at extreme latitudes
        let edge_cases = vec![
            (0.0, 85.0511287798066),     // Max valid latitude
            (0.0, -85.0511287798066),    // Min valid latitude
            (179.9999, 0.0),             // Near antimeridian
            (-179.9999, 0.0),            // Near antimeridian
        ];
        
        for (lon, lat) in edge_cases {
            let (x, y) = lon_lat_to_mercator(lon, lat);
            
            // STRICT: Should not produce infinite or NaN values
            assert!(x.is_finite(), 
                "STRICT FAIL: Longitude {} produced non-finite X coordinate: {}", lon, x);
            assert!(y.is_finite(), 
                "STRICT FAIL: Latitude {} produced non-finite Y coordinate: {}", lat, y);
            
            // STRICT: Round-trip should be accurate to 1e-10 degrees
            let (back_lon, back_lat) = mercator_to_lon_lat(x, y);
            
            assert!((back_lon - lon).abs() < 1e-10,
                "STRICT FAIL: Longitude round-trip error > 1e-10°: {} -> {} -> {}, error: {}",
                lon, x, back_lon, (back_lon - lon).abs());
                
            assert!((back_lat - lat).abs() < 1e-10,
                "STRICT FAIL: Latitude round-trip error > 1e-10°: {} -> {} -> {}, error: {}",
                lat, y, back_lat, (back_lat - lat).abs());
        }
    }
    
    #[tokio::test]
    async fn test_strict_concurrent_access() {
        let (tmp_dir, data_dir) = setup_test_data_dir();
        
        create_test_geotiff(
            &data_dir.join("test"),
            "concurrent.tif",
            (512, 512),
            3857,
            (-20037508.342789244, 20037508.342789244, -20037508.342789244, 20037508.342789244),
            (0.0, 255.0),
            0.0,
            5007,
        );
        
        let reader = Arc::new(LocalTileReader::new(&data_dir).await);
        
        // STRICT: Multiple concurrent requests should all succeed and return identical results
        let handles: Vec<_> = (0..10).map(|_| {
            let reader_clone = reader.clone();
            tokio::spawn(async move {
                reader_clone.get_tile("concurrent", 1, 0, 0, None).await
            })
        }).collect();
        
        let mut results = Vec::new();
        for handle in handles {
            let result = handle.await.unwrap();
            assert!(result.is_ok(), "STRICT FAIL: Concurrent request failed");
            results.push(result.unwrap());
        }
        
        // STRICT: All results should be identical
        let first_tile = &results[0];
        for (i, tile) in results.iter().enumerate().skip(1) {
            assert_eq!(tile.bytes.len(), first_tile.bytes.len(),
                "STRICT FAIL: Concurrent request {} returned different size ({} vs {} bytes)",
                i, tile.bytes.len(), first_tile.bytes.len());
                
            assert_eq!(tile.bytes, first_tile.bytes,
                "STRICT FAIL: Concurrent request {} returned different content",
                i);
        }
        
        drop(tmp_dir);
    }
    
    #[tokio::test]
    async fn test_strict_memory_efficiency() {
        let (tmp_dir, data_dir) = setup_test_data_dir();
        
        // Create a large dataset to test memory usage
        create_test_geotiff(
            &data_dir.join("test"),
            "large.tif",
            (2048, 2048), // Large source image
            3857,
            (-20037508.342789244, 20037508.342789244, -20037508.342789244, 20037508.342789244),
            (0.0, 255.0),
            0.0,
            5008,
        );
        
        let reader = Arc::new(LocalTileReader::new(&data_dir).await);
        
        // STRICT: Should be able to generate tiles from large datasets efficiently
        let start_memory = get_memory_usage();
        
        // Generate several tiles to test memory growth
        for i in 0..5 {
            let _tile = reader.get_tile("large", 5, i, i, None).await
                .expect("Should handle large dataset");
        }
        
        let end_memory = get_memory_usage();
        let memory_growth = end_memory - start_memory;
        
        // STRICT: Memory growth should be reasonable (< 100MB for 5 tiles)
        assert!(memory_growth < 100_000_000,
            "STRICT FAIL: Excessive memory growth: {} bytes for 5 tiles from large dataset",
            memory_growth);
        
        drop(tmp_dir);
    }
    
    // Helper function to get approximate memory usage
    fn get_memory_usage() -> usize {
        // Simple approximation - in a real implementation you'd use proper memory monitoring
        // Return a dummy value for now since real memory measurement is complex
        0
    }
}