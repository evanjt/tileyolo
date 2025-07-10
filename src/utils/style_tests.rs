#[cfg(test)]
mod style_system_tests {
    use crate::test_utils::test_helpers::*;
    use crate::reader::local::LocalTileReader;
    use crate::traits::TileReader;
    use crate::utils::style::parse_style_file;
    use tempfile::TempDir;
    use std::sync::Arc;
    use image::{ImageDecoder, codecs::png::PngDecoder, ColorType};
    use std::io::Cursor;

    #[tokio::test]
    async fn test_builtin_gradient_styles() {
        let (tmp_dir, data_dir) = setup_test_data_dir();
        
        // Test all built-in gradients
        let gradients = vec!["viridis", "magma", "plasma", "inferno", "turbo", "rainbow", "cubehelix"];
        
        for gradient in &gradients {
            create_test_geotiff(
                &data_dir.join(gradient),
                &format!("{}.tif", gradient),
                (100, 100),
                4326,
                (-10.0, 10.0, -10.0, 10.0),
                (0.0, 100.0),
                0.0,
                50 + gradients.iter().position(|&g| g == *gradient).unwrap() as u64,
            );
        }
        
        let reader = Arc::new(LocalTileReader::new(&data_dir).await);
        let layers = reader.list_layers().await;
        
        // Verify all gradient styles are loaded
        assert_eq!(layers.len(), gradients.len());
        for gradient in &gradients {
            assert!(layers.iter().any(|l| l.style == *gradient));
        }
        
        // Test tile generation with each style
        for gradient in &gradients {
            let result = reader.get_tile(gradient, 0, 0, 0, None).await;
            assert!(result.is_ok(), "Failed to generate tile for {} style", gradient);
            
            let tile = result.unwrap();
            let decoder = PngDecoder::new(Cursor::new(&tile.bytes)).unwrap();
            assert_eq!(decoder.color_type(), ColorType::Rgba8);
        }
        
        drop(tmp_dir);
    }
    
    #[tokio::test]
    async fn test_custom_style_file() {
        let (tmp_dir, data_dir) = setup_test_data_dir();
        
        // Create custom style with specific color stops
        let custom_stops = vec![
            (0.0, 255, 0, 0),      // Red at min
            (25.0, 255, 255, 0),   // Yellow
            (50.0, 0, 255, 0),     // Green
            (75.0, 0, 255, 255),   // Cyan
            (100.0, 0, 0, 255),    // Blue at max
        ];
        
        create_test_style_file(&data_dir.join("custom"), custom_stops.clone());
        
        create_test_geotiff(
            &data_dir.join("custom"),
            "custom_styled.tif",
            (100, 100),
            4326,
            (-10.0, 10.0, -10.0, 10.0),
            (0.0, 100.0),
            0.0,
            60,
        );
        
        let reader = Arc::new(LocalTileReader::new(&data_dir).await);
        let layers = reader.list_layers().await;
        
        // Verify custom style is loaded
        assert!(layers.iter().any(|l| l.layer == "custom_styled" && l.style == "custom"));
        
        // Test tile generation
        let result = reader.get_tile("custom_styled", 0, 0, 0, None).await;
        assert!(result.is_ok());
        
        drop(tmp_dir);
    }
    
    #[tokio::test]
    async fn test_grayscale_fallback() {
        let (tmp_dir, data_dir) = setup_test_data_dir();
        
        // Create data in grayscale folder (no style.txt)
        create_test_geotiff(
            &data_dir.join("grayscale"),
            "gray_data.tif",
            (100, 100),
            4326,
            (-10.0, 10.0, -10.0, 10.0),
            (0.0, 255.0),
            0.1,
            61,
        );
        
        let reader = Arc::new(LocalTileReader::new(&data_dir).await);
        let layers = reader.list_layers().await;
        
        // Verify grayscale style
        assert!(layers.iter().any(|l| l.layer == "gray_data" && l.style == "grayscale"));
        
        let result = reader.get_tile("gray_data", 0, 0, 0, None).await;
        assert!(result.is_ok());
        
        drop(tmp_dir);
    }
    
    #[test]
    fn test_style_loading_from_file() {
        let tmp_dir = TempDir::new().unwrap();
        
        // Test valid style file
        let stops = vec![
            (0.0, 0, 0, 0),
            (50.0, 128, 128, 128),
            (100.0, 255, 255, 255),
        ];
        
        let style_path = create_test_style_file(tmp_dir.path(), stops);
        let loaded_style = parse_style_file(&style_path);
        
        assert!(loaded_style.is_ok());
        let color_stops = loaded_style.unwrap();
        assert_eq!(color_stops.len(), 3);
        
        // Test invalid style file (invalid number format)
        std::fs::write(tmp_dir.path().join("bad_style.txt"), "not_a_number,255,0,0,255\n").unwrap();
        let bad_style = parse_style_file(&tmp_dir.path().join("bad_style.txt"));
        assert!(bad_style.is_err());
        
        drop(tmp_dir);
    }
    
    #[tokio::test]
    async fn test_nodata_transparency() {
        let (tmp_dir, data_dir) = setup_test_data_dir();
        
        // Create data with high nodata fraction
        create_test_geotiff(
            &data_dir.join("viridis"),
            "nodata_test.tif",
            (100, 100),
            4326,
            (-10.0, 10.0, -10.0, 10.0),
            (0.0, 100.0),
            0.5, // 50% nodata
            62,
        );
        
        let reader = Arc::new(LocalTileReader::new(&data_dir).await);
        let result = reader.get_tile("nodata_test", 0, 0, 0, None).await;
        assert!(result.is_ok());
        
        let tile = result.unwrap();
        let decoder = PngDecoder::new(Cursor::new(&tile.bytes)).unwrap();
        
        // Ensure RGBA format for transparency
        assert_eq!(decoder.color_type(), ColorType::Rgba8);
        
        drop(tmp_dir);
    }
}