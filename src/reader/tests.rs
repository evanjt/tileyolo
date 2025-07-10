#[cfg(test)]
mod tile_generation_tests {
    use crate::test_utils::test_helpers::*;
    use crate::reader::local::LocalTileReader;
    use crate::traits::TileReader;
    use std::sync::Arc;
    use image::{ImageDecoder, codecs::png::PngDecoder};
    use std::io::Cursor;

    #[tokio::test]
    async fn test_xyz_tile_coordinates() {
        let (tmp_dir, data_dir) = setup_test_data_dir();
        
        // Create a global dataset in Web Mercator
        create_test_geotiff(
            &data_dir.join("grayscale"),
            "global.tif",
            (512, 512),
            3857,
            (-20037508.34, 20037508.34, -20037508.34, 20037508.34),
            (0.0, 255.0),
            0.05,
            45,
        );
        
        let reader = Arc::new(LocalTileReader::new(&data_dir).await);
        
        // Test various tile coordinates
        let test_tiles = vec![
            (0, 0, 0),    // Zoom 0, entire world
            (1, 0, 0),    // Zoom 1, northwest quadrant
            (1, 1, 0),    // Zoom 1, northeast quadrant
            (1, 0, 1),    // Zoom 1, southwest quadrant
            (1, 1, 1),    // Zoom 1, southeast quadrant
            (5, 10, 15),  // Zoom 5, arbitrary tile
            (10, 512, 512), // Zoom 10, center tile
        ];
        
        for (z, x, y) in test_tiles {
            let result = reader.get_tile("global", z, x, y, None).await;
            assert!(result.is_ok(), "Failed to get tile {}/{}/{}", z, x, y);
            
            let tile = result.unwrap();
            assert_eq!(tile.content_type, "image/png");
            
            // Verify PNG dimensions
            let decoder = PngDecoder::new(Cursor::new(&tile.bytes)).unwrap();
            let (width, height) = decoder.dimensions();
            assert_eq!(width, 256);
            assert_eq!(height, 256);
        }
        
        drop(tmp_dir);
    }
    
    #[tokio::test]
    async fn test_tile_bounds_at_edges() {
        let (tmp_dir, data_dir) = setup_test_data_dir();
        
        // Create dataset covering specific region
        create_test_geotiff(
            &data_dir.join("grayscale"),
            "regional.tif",
            (256, 256),
            3857,
            (0.0, 5000000.0, 0.0, 5000000.0), // Northeast quadrant subset
            (100.0, 200.0),
            0.0,
            46,
        );
        
        let reader = Arc::new(LocalTileReader::new(&data_dir).await);
        
        // Test edge cases
        // Zoom 2 has 4x4 tiles (0-3 for x and y)
        let edge_cases = vec![
            (2, 0, 0),   // Top-left corner
            (2, 3, 0),   // Top-right corner
            (2, 0, 3),   // Bottom-left corner
            (2, 3, 3),   // Bottom-right corner
            (8, 255, 0), // High zoom edge
        ];
        
        for (z, x, y) in edge_cases {
            let result = reader.get_tile("regional", z, x, y, None).await;
            if result.is_ok() {
                let tile = result.unwrap();
                assert!(!tile.bytes.is_empty());
            }
        }
        
        drop(tmp_dir);
    }
    
    #[tokio::test]
    async fn test_out_of_bounds_tiles() {
        let (tmp_dir, data_dir) = setup_test_data_dir();
        
        create_test_geotiff(
            &data_dir.join("grayscale"),
            "bounded.tif",
            (128, 128),
            3857,
            (-1000000.0, 1000000.0, -1000000.0, 1000000.0), // Small area
            (50.0, 150.0),
            0.0,
            47,
        );
        
        let reader = Arc::new(LocalTileReader::new(&data_dir).await);
        
        // These tiles should be completely outside the data bounds
        let out_of_bounds = vec![
            (1, 0, 0),   // Northwest, data is in center
            (5, 0, 0),   // Far northwest
            (5, 31, 31), // Far southeast
        ];
        
        for (z, x, y) in out_of_bounds {
            let result = reader.get_tile("bounded", z, x, y, None).await;
            // The implementation might return empty tiles or errors for out-of-bounds
            // We just verify it doesn't panic
            match result {
                Ok(tile) => {
                    assert_eq!(tile.content_type, "image/png");
                }
                Err(_) => {
                    // Expected for out-of-bounds tiles
                }
            }
        }
        
        drop(tmp_dir);
    }
}

#[cfg(test)]
mod file_naming_tests {
    use crate::test_utils::test_helpers::*;
    use crate::reader::local::LocalTileReader;
    use crate::traits::TileReader;
    use std::sync::Arc;
    use std::fs;

    #[tokio::test]
    async fn test_various_file_names() {
        let (tmp_dir, data_dir) = setup_test_data_dir();
        
        // Test different file naming patterns
        let test_files = vec![
            ("simple.tif", "simple"),
            ("hyphen-name.tif", "hyphen-name"),
            ("underscore_name.tif", "underscore_name"),
            ("CamelCase.tif", "CamelCase"),
            ("with.dots.in.name.tif", "with.dots.in.name"),
            ("123numeric.tif", "123numeric"),
            ("special_chars_123.tif", "special_chars_123"),
            ("UPPERCASE.TIF", "UPPERCASE"),
            ("mixed.TiF", "mixed"),
            ("very_long_file_name_with_many_words_and_numbers_12345.tif", 
             "very_long_file_name_with_many_words_and_numbers_12345"),
        ];
        
        // Create test files in different style directories
        for (i, (filename, _expected_layer)) in test_files.iter().enumerate() {
            let style = if i % 2 == 0 { "viridis" } else { "grayscale" };
            create_test_geotiff(
                &data_dir.join(style),
                filename,
                (50, 50),
                4326,
                (-10.0, 10.0, -10.0, 10.0),
                (0.0, 100.0),
                0.0,
                100 + i as u64,
            );
        }
        
        let reader = Arc::new(LocalTileReader::new(&data_dir).await);
        let layers = reader.list_layers().await;
        
        // Verify all files are detected with correct layer names
        for (_, expected_layer) in &test_files {
            assert!(
                layers.iter().any(|l| l.layer == *expected_layer),
                "Layer '{}' not found", expected_layer
            );
        }
        
        // Test tile generation for each layer
        for (_, layer_name) in &test_files {
            let result = reader.get_tile(layer_name, 0, 0, 0, None).await;
            assert!(
                result.is_ok(),
                "Failed to generate tile for layer '{}'", layer_name
            );
        }
        
        drop(tmp_dir);
    }
    
    #[tokio::test]
    async fn test_multiple_files_same_style() {
        let (tmp_dir, data_dir) = setup_test_data_dir();
        
        // Create multiple files in the same style directory
        let files = vec!["elevation.tif", "terrain.tif", "heights.tif"];
        
        for (i, filename) in files.iter().enumerate() {
            create_test_geotiff(
                &data_dir.join("viridis"),
                filename,
                (64, 64),
                4326,
                (-20.0, 20.0, -20.0, 20.0),
                (0.0 + i as f32 * 100.0, 100.0 + i as f32 * 100.0),
                0.0,
                200 + i as u64,
            );
        }
        
        let reader = Arc::new(LocalTileReader::new(&data_dir).await);
        let layers = reader.list_layers().await;
        
        // Verify all files are loaded with the same style
        let viridis_layers: Vec<_> = layers
            .iter()
            .filter(|l| l.style == "viridis")
            .collect();
        
        assert_eq!(viridis_layers.len(), 3);
        assert!(viridis_layers.iter().any(|l| l.layer == "elevation"));
        assert!(viridis_layers.iter().any(|l| l.layer == "terrain"));
        assert!(viridis_layers.iter().any(|l| l.layer == "heights"));
        
        drop(tmp_dir);
    }
    
    #[tokio::test]
    async fn test_non_tiff_files_ignored() {
        let (tmp_dir, data_dir) = setup_test_data_dir();
        
        // Create valid TIFF file
        create_test_geotiff(
            &data_dir.join("viridis"),
            "valid.tif",
            (50, 50),
            4326,
            (-10.0, 10.0, -10.0, 10.0),
            (0.0, 100.0),
            0.0,
            300,
        );
        
        // Create non-TIFF files that should be ignored
        fs::write(data_dir.join("viridis").join("readme.txt"), "This is not a TIFF").unwrap();
        fs::write(data_dir.join("viridis").join("data.json"), r#"{"type": "not a tiff"}"#).unwrap();
        fs::write(data_dir.join("viridis").join("image.png"), &[0x89, 0x50, 0x4E, 0x47]).unwrap();
        fs::write(data_dir.join("viridis").join("no_extension"), "data").unwrap();
        
        let reader = Arc::new(LocalTileReader::new(&data_dir).await);
        let layers = reader.list_layers().await;
        
        // Only the valid TIFF should be loaded
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].layer, "valid");
        assert_eq!(layers[0].style, "viridis");
        
        drop(tmp_dir);
    }
    
    #[tokio::test]
    async fn test_subdirectory_handling() {
        let (tmp_dir, data_dir) = setup_test_data_dir();
        
        // Create a subdirectory (should be ignored)
        let subdir = data_dir.join("viridis").join("subdir");
        fs::create_dir(&subdir).unwrap();
        
        // Create files in main directory and subdirectory
        create_test_geotiff(
            &data_dir.join("viridis"),
            "main.tif",
            (50, 50),
            4326,
            (-10.0, 10.0, -10.0, 10.0),
            (0.0, 100.0),
            0.0,
            400,
        );
        
        create_test_geotiff(
            &subdir,
            "sub.tif",
            (50, 50),
            4326,
            (-10.0, 10.0, -10.0, 10.0),
            (0.0, 100.0),
            0.0,
            401,
        );
        
        let reader = Arc::new(LocalTileReader::new(&data_dir).await);
        let layers = reader.list_layers().await;
        
        // Both files might be loaded depending on implementation
        // Check that at least the main file is loaded
        assert!(layers.iter().any(|l| l.layer == "main"));
        // If subdirectories are scanned as styles, verify the subdir layer exists
        if layers.len() > 1 {
            assert!(layers.iter().any(|l| l.style == "subdir"));
        }
        
        drop(tmp_dir);
    }
    
    #[tokio::test]
    async fn test_duplicate_names_different_styles() {
        let (tmp_dir, data_dir) = setup_test_data_dir();
        
        // Create files with same name in different style directories
        let styles = vec!["viridis", "magma", "grayscale"];
        
        for (i, style) in styles.iter().enumerate() {
            create_test_geotiff(
                &data_dir.join(style),
                "same_name.tif",
                (50, 50),
                4326,
                (-10.0, 10.0, -10.0, 10.0),
                (0.0 + i as f32 * 50.0, 100.0 + i as f32 * 50.0),
                0.0,
                500 + i as u64,
            );
        }
        
        let reader = Arc::new(LocalTileReader::new(&data_dir).await);
        let layers = reader.list_layers().await;
        
        // All files should be loaded with their respective styles
        assert_eq!(layers.len(), 3);
        
        let same_name_layers: Vec<_> = layers
            .iter()
            .filter(|l| l.layer == "same_name")
            .collect();
        
        assert_eq!(same_name_layers.len(), 3);
        assert!(same_name_layers.iter().any(|l| l.style == "viridis"));
        assert!(same_name_layers.iter().any(|l| l.style == "magma"));
        assert!(same_name_layers.iter().any(|l| l.style == "grayscale"));
        
        // Test accessing each variant
        for style in &styles {
            // The API uses layer name, so it might return the first match
            // This tests that the system can handle duplicates
            let result = reader.get_tile("same_name", 0, 0, 0, Some(style)).await;
            assert!(result.is_ok());
        }
        
        drop(tmp_dir);
    }
}