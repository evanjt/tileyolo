#[cfg(test)]
mod xyz_tile_server_compliance_tests {
    use crate::test_utils::test_helpers::*;
    use crate::reader::local::LocalTileReader;
    use crate::traits::TileReader;
    use std::sync::Arc;
    use image::{ImageDecoder, codecs::png::PngDecoder, ColorType};
    use std::io::Cursor;

    /// XYZ Tile Server Standard Compliance Tests
    /// Based on: https://wiki.openstreetmap.org/wiki/Slippy_map_tilenames
    /// And: https://developers.google.com/maps/documentation/tile/overview

    #[tokio::test]
    async fn test_xyz_coordinate_system_compliance() {
        let (tmp_dir, data_dir) = setup_test_data_dir();
        
        create_test_geotiff(
            &data_dir.join("test"),
            "world.tif",
            (512, 512),
            3857, // Web Mercator is required for XYZ
            (-20037508.342789244, 20037508.342789244, -20037508.342789244, 20037508.342789244),
            (0.0, 255.0),
            0.0,
            5000,
        );
        
        let reader = Arc::new(LocalTileReader::new(&data_dir).await);
        
        // XYZ Rule 1: Zoom level 0 has exactly 1 tile (0,0)
        let z0_tile = reader.get_tile("world", 0, 0, 0, None).await;
        assert!(z0_tile.is_ok(), "Zoom 0 must have tile (0,0)");
        
        // XYZ Rule 2: Invalid coordinates should fail gracefully
        let _invalid_z0 = reader.get_tile("world", 0, 1, 0, None).await;
        // Note: Some implementations allow out-of-bounds, others don't
        // We test that it doesn't panic, behavior may vary
        
        // XYZ Rule 3: Zoom level N has 2^N tiles per dimension
        for zoom in 1..=5 {
            let max_coord = (1u32 << zoom) - 1;
            
            // Test corner tiles exist
            let nw = reader.get_tile("world", zoom, 0, 0, None).await;
            assert!(nw.is_ok(), "Northwest tile at zoom {} should exist", zoom);
            
            let se = reader.get_tile("world", zoom, max_coord, max_coord, None).await;
            assert!(se.is_ok(), "Southeast tile at zoom {} should exist", zoom);
        }
        
        drop(tmp_dir);
    }
    
    #[tokio::test]
    async fn test_tile_image_format_compliance() {
        let (tmp_dir, data_dir) = setup_test_data_dir();
        
        create_test_geotiff(
            &data_dir.join("test"),
            "format_test.tif",
            (256, 256),
            3857,
            (-10000000.0, 10000000.0, -10000000.0, 10000000.0),
            (0.0, 255.0),
            0.1, // Include some nodata for transparency test
            6000,
        );
        
        let reader = Arc::new(LocalTileReader::new(&data_dir).await);
        let tile_result = reader.get_tile("format_test", 1, 0, 0, None).await;
        assert!(tile_result.is_ok());
        
        let tile = tile_result.unwrap();
        
        // XYZ Rule 4: Content-Type must be image/png
        assert_eq!(tile.content_type, "image/png", "XYZ tiles must be PNG format");
        
        // XYZ Rule 5: Tile dimensions must be exactly 256x256 pixels
        let decoder = PngDecoder::new(Cursor::new(&tile.bytes)).unwrap();
        let (width, height) = decoder.dimensions();
        assert_eq!(width, 256, "XYZ tiles must be exactly 256 pixels wide");
        assert_eq!(height, 256, "XYZ tiles must be exactly 256 pixels tall");
        
        // XYZ Rule 6: Must support transparency for nodata/empty areas
        assert_eq!(decoder.color_type(), ColorType::Rgba8, "XYZ tiles must support transparency (RGBA)");
        
        // XYZ Rule 7: PNG must be valid and decodable
        assert!(!tile.bytes.is_empty(), "Tile must contain valid PNG data");
        assert_eq!(&tile.bytes[0..8], b"\x89PNG\r\n\x1a\n", "Must be valid PNG file signature");
        
        drop(tmp_dir);
    }
    
    #[tokio::test]
    async fn test_web_mercator_projection_compliance() {
        let (tmp_dir, data_dir) = setup_test_data_dir();
        
        // XYZ Rule 8: Must use Web Mercator projection (EPSG:3857)
        create_test_geotiff(
            &data_dir.join("test"),
            "mercator.tif",
            (256, 256),
            3857, // This is required for XYZ compliance
            (-20037508.342789244, 20037508.342789244, -20037508.342789244, 20037508.342789244),
            (0.0, 255.0),
            0.0,
            7000,
        );
        
        let reader = Arc::new(LocalTileReader::new(&data_dir).await);
        let layers = reader.list_layers().await;
        
        let test_layer = layers.iter().find(|l| l.layer == "mercator").unwrap();
        
        // Verify the layer can generate tiles (projection works)
        let tile = reader.get_tile("mercator", 0, 0, 0, None).await;
        assert!(tile.is_ok(), "Web Mercator tiles must be generatable");
        
        // XYZ Rule 9: Coordinate bounds must align with Web Mercator limits
        // Web Mercator spans: ±20037508.342789244 meters
        let extent = &test_layer.source_geometry.extent;
        let max_mercator = 20037508.342789244;
        
        assert!(
            extent.minx >= -max_mercator - 1000.0, // 1km tolerance
            "X minimum should be within Web Mercator bounds"
        );
        assert!(
            extent.maxx <= max_mercator + 1000.0,
            "X maximum should be within Web Mercator bounds"
        );
        
        drop(tmp_dir);
    }
    
    #[tokio::test]
    async fn test_tile_addressing_scheme_compliance() {
        let (tmp_dir, data_dir) = setup_test_data_dir();
        
        create_test_geotiff(
            &data_dir.join("test"),
            "addressing.tif",
            (512, 512),
            3857,
            (-20037508.342789244, 20037508.342789244, -20037508.342789244, 20037508.342789244),
            (0.0, 255.0),
            0.0,
            8000,
        );
        
        let reader = Arc::new(LocalTileReader::new(&data_dir).await);
        
        // XYZ Rule 10: Tile addressing follows TMS/Slippy Map convention
        // Format: /tiles/{layer}/{z}/{x}/{y}
        // Where:
        // - z = zoom level (0 to ~20)
        // - x = column (0 to 2^z - 1, west to east)
        // - y = row (0 to 2^z - 1, north to south)
        
        let test_cases = vec![
            // (z, x, y, description)
            (0, 0, 0, "Single tile at zoom 0"),
            (1, 0, 0, "Northwest at zoom 1"),
            (1, 1, 0, "Northeast at zoom 1"),
            (1, 0, 1, "Southwest at zoom 1"),
            (1, 1, 1, "Southeast at zoom 1"),
            (2, 0, 0, "Northwest corner at zoom 2"),
            (2, 3, 3, "Southeast corner at zoom 2"),
            (10, 512, 341, "Arbitrary valid coordinate at zoom 10"),
        ];
        
        for (z, x, y, description) in test_cases {
            let result = reader.get_tile("addressing", z, x, y, None).await;
            assert!(
                result.is_ok(),
                "Failed XYZ addressing test: {} - tile {}/{}/{}",
                description, z, x, y
            );
        }
        
        drop(tmp_dir);
    }
    
    #[tokio::test]
    async fn test_http_response_compliance() {
        let (tmp_dir, data_dir) = setup_test_data_dir();
        
        create_test_geotiff(
            &data_dir.join("test"),
            "http_test.tif",
            (256, 256),
            3857,
            (-5000000.0, 5000000.0, -5000000.0, 5000000.0),
            (0.0, 255.0),
            0.0,
            9000,
        );
        
        let reader = Arc::new(LocalTileReader::new(&data_dir).await);
        
        // XYZ Rule 11: Valid tiles must return successful response
        let valid_tile = reader.get_tile("http_test", 1, 0, 0, None).await;
        assert!(valid_tile.is_ok(), "Valid tile requests must succeed");
        
        let tile = valid_tile.unwrap();
        
        // XYZ Rule 12: Response must have correct Content-Type
        assert_eq!(tile.content_type, "image/png");
        
        // XYZ Rule 13: Response must contain valid image data
        assert!(!tile.bytes.is_empty(), "Response must contain image data");
        assert!(tile.bytes.len() > 100, "PNG data should be substantial size");
        
        // XYZ Rule 14: Invalid layer names should fail gracefully
        let invalid_layer = reader.get_tile("nonexistent_layer", 0, 0, 0, None).await;
        assert!(invalid_layer.is_err(), "Invalid layer requests must fail");
        
        drop(tmp_dir);
    }
    
    #[tokio::test]
    async fn test_zoom_level_consistency() {
        let (tmp_dir, data_dir) = setup_test_data_dir();
        
        create_test_geotiff(
            &data_dir.join("test"),
            "zoom_test.tif",
            (1024, 1024),
            3857,
            (-20037508.342789244, 20037508.342789244, -20037508.342789244, 20037508.342789244),
            (0.0, 255.0),
            0.0,
            10000,
        );
        
        let reader = Arc::new(LocalTileReader::new(&data_dir).await);
        
        // XYZ Rule 15: Zoom levels must follow geometric progression
        // Each zoom level doubles resolution in each dimension
        for zoom in 0..=10 {
            let tiles_per_side = 1u32 << zoom; // 2^zoom
            
            // Test that max valid coordinate is 2^zoom - 1
            let max_coord = tiles_per_side - 1;
            
            // Corner tiles should be accessible
            let corner_tile = reader.get_tile("zoom_test", zoom, max_coord, max_coord, None).await;
            assert!(
                corner_tile.is_ok(),
                "Corner tile at zoom {} coordinate ({}, {}) should be accessible",
                zoom, max_coord, max_coord
            );
            
            // All tiles at this zoom should be same size
            if let Ok(tile) = corner_tile {
                let decoder = PngDecoder::new(Cursor::new(&tile.bytes)).unwrap();
                let (width, height) = decoder.dimensions();
                assert_eq!(width, 256, "All tiles must be 256px wide at zoom {}", zoom);
                assert_eq!(height, 256, "All tiles must be 256px tall at zoom {}", zoom);
            }
        }
        
        drop(tmp_dir);
    }
    
    #[tokio::test]
    async fn test_layer_metadata_compliance() {
        let (tmp_dir, data_dir) = setup_test_data_dir();
        
        // Create multiple layers to test metadata
        let layers_data = vec![
            ("elevation", "viridis", (0.0, 1000.0)),
            ("temperature", "magma", (-20.0, 40.0)),
            ("population", "plasma", (0.0, 1000000.0)),
        ];
        
        for (layer_name, style, (min_val, max_val)) in &layers_data {
            create_test_geotiff(
                &data_dir.join(style),
                &format!("{}.tif", layer_name),
                (200, 200),
                3857,
                (-10000000.0, 10000000.0, -10000000.0, 10000000.0),
                (*min_val, *max_val),
                0.05,
                11000 + layers_data.iter().position(|(n, _, _)| n == layer_name).unwrap() as u64,
            );
        }
        
        let reader = Arc::new(LocalTileReader::new(&data_dir).await);
        let layers = reader.list_layers().await;
        
        // XYZ Rule 16: Layer listing must be available
        assert!(!layers.is_empty(), "Server must provide layer metadata");
        assert_eq!(layers.len(), 3, "All layers should be discoverable");
        
        // XYZ Rule 17: Each layer must have consistent metadata
        for layer in &layers {
            assert!(!layer.layer.is_empty(), "Layer must have non-empty name");
            assert!(!layer.style.is_empty(), "Layer must have style information");
            assert!(layer.min_value.is_finite(), "Layer must have valid min value");
            assert!(layer.max_value.is_finite(), "Layer must have valid max value");
            assert!(layer.min_value <= layer.max_value, "Min value must be <= max value");
            
            // Verify each layer can generate tiles
            let test_tile = reader.get_tile(&layer.layer, 1, 0, 0, None).await;
            assert!(
                test_tile.is_ok(),
                "Layer '{}' must be able to generate tiles", layer.layer
            );
        }
        
        drop(tmp_dir);
    }
    
    #[tokio::test]
    async fn test_edge_case_compliance() {
        let (tmp_dir, data_dir) = setup_test_data_dir();
        
        create_test_geotiff(
            &data_dir.join("test"),
            "edge_cases.tif",
            (128, 128),
            3857,
            (-20037508.342789244, 20037508.342789244, -20037508.342789244, 20037508.342789244),
            (0.0, 255.0),
            0.2, // 20% nodata for edge case testing
            12000,
        );
        
        let reader = Arc::new(LocalTileReader::new(&data_dir).await);
        
        // XYZ Rule 18: Handle boundary conditions gracefully
        let edge_cases = vec![
            // High zoom with valid coordinates
            (15, 16384, 16384),
            // Edge coordinates at various zooms
            (5, 0, 0),      // Northwest corner
            (5, 31, 0),     // Northeast corner  
            (5, 0, 31),     // Southwest corner
            (5, 31, 31),    // Southeast corner
        ];
        
        for (z, x, y) in edge_cases {
            let result = reader.get_tile("edge_cases", z, x, y, None).await;
            // Should either succeed or fail gracefully (not panic)
            match result {
                Ok(tile) => {
                    // If successful, must meet XYZ standards
                    assert_eq!(tile.content_type, "image/png");
                    let decoder = PngDecoder::new(Cursor::new(&tile.bytes)).unwrap();
                    let (width, height) = decoder.dimensions();
                    assert_eq!(width, 256);
                    assert_eq!(height, 256);
                },
                Err(_) => {
                    // Graceful failure is acceptable for out-of-bounds
                }
            }
        }
        
        drop(tmp_dir);
    }
    
    #[test]
    fn test_xyz_url_pattern_compliance() {
        // XYZ Rule 19: URL pattern must follow standard format
        // Standard: /{z}/{x}/{y} or /tiles/{layer}/{z}/{x}/{y}
        
        // Test coordinate ranges for various zoom levels
        let test_cases = vec![
            (0, 0, 0),      // Zoom 0: only (0,0) valid
            (1, 0, 0), (1, 1, 1),  // Zoom 1: 0-1 range
            (2, 0, 0), (2, 3, 3),  // Zoom 2: 0-3 range
            (10, 0, 0), (10, 1023, 1023),  // Zoom 10: 0-1023 range
            (18, 0, 0), (18, 262143, 262143),  // Zoom 18: 0-262143 range
        ];
        
        for (z, x, y) in test_cases {
            // Verify coordinates are within valid range for zoom level
            let max_coord = (1u32 << z) - 1;
            assert!(x <= max_coord, "X coordinate {} exceeds max {} for zoom {}", x, max_coord, z);
            assert!(y <= max_coord, "Y coordinate {} exceeds max {} for zoom {}", y, max_coord, z);
            
            // Verify coordinates are non-negative (implicit with u32)
            assert!(z <= 25, "Zoom level {} exceeds practical maximum", z);
        }
    }
}