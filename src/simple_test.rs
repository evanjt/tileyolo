#[cfg(test)]
mod tests {
    use crate::models::{
        geometry::GeometryExtent,
        layer::{Layer, LayerGeometry},
    };
    use crate::reader::cog::process_cog;
    use std::path::PathBuf;

    async fn make_simple_layer() -> Layer {
        let path = PathBuf::from("nonexistent.tif"); // We don't need actual file for basic test

        let source_geometry = LayerGeometry {
            crs_code: 3857,
            extent: GeometryExtent {
                minx: 0.0,
                miny: 0.0,
                maxx: 256.0,
                maxy: 256.0,
            },
        };
        let cached_geometry = source_geometry.generate_cached_geometry_sync().unwrap();

        Layer {
            layer: "test".to_string(),
            style: "default".to_string(),
            path,
            size_bytes: 0,
            source_geometry,
            cached_geometry,
            colour_stops: vec![],
            min_value: 0.0,
            max_value: 100.0,
            is_cog: true,
            last_modified: std::time::SystemTime::UNIX_EPOCH,
            bands: 1,
        }
    }

    #[tokio::test]
    async fn test_cog_basic_functionality() {
        // This test just verifies the function signature works
        // It will fail when trying to read the file, but that's expected
        let layer = make_simple_layer().await;
        let extent = GeometryExtent {
            minx: 0.0,
            miny: 0.0,
            maxx: 256.0,
            maxy: 256.0,
        };

        let result = process_cog(layer.path.clone(), extent, layer, (256, 256)).await;

        // We expect this to fail because the file doesn't exist
        assert!(result.is_err());
    }

    #[test]
    fn test_world_to_pixel_conversion() {
        use crate::reader::cog::world_to_pixel;
        use geo::{AffineTransform, Coord};

        let transform = AffineTransform::new(1.0, 0.0, 0.0, 0.0, 1.0, 0.0);
        let world_coord = Coord { x: 10.0, y: 20.0 };
        let pixel_coord = world_to_pixel(world_coord, &transform);

        assert!((pixel_coord.x - 10.0).abs() < f64::EPSILON);
        assert!((pixel_coord.y - 20.0).abs() < f64::EPSILON);
    }
}
