use std::f64::consts::PI;
use proj::Proj;

/// `WebMercator` constants
const R_MAJOR: f64 = 6378137.0;
const MAX_LAT: f64 = 85.05112877980659; // Max bounds for Web Mercator

/// Create a coordinate transformer from EPSG:3857 (Web Mercator) to a target EPSG code
/// Returns None if source and target are the same (no transform needed), or an error message if transform fails
pub fn create_transformer(source_epsg: u32) -> Result<Option<Proj>, String> {
    // If source is already 3857, no transform needed
    if source_epsg == 3857 {
        return Ok(None);
    }

    let source_crs = format!("EPSG:{source_epsg}");

    // Create transformer from 3857 to source CRS
    Proj::new_known_crs("EPSG:3857", &source_crs, None)
        .map(Some)
        .map_err(|e| format!("Failed to create transformer from EPSG:3857 to {source_crs}: {e}"))
}

/// Transform coordinates from EPSG:3857 to the source CRS
/// If transformer is None (source is 3857), returns coordinates unchanged
pub fn transform_coords(transformer: &Option<Proj>, x: f64, y: f64) -> (f64, f64) {
    match transformer {
        Some(proj) => proj.convert((x, y)).unwrap_or((x, y)),
        None => (x, y),
    }
}

/// from longitude, latitude (degrees) → Web Mercator (x, y in meters)
pub fn lon_lat_to_mercator(lon: f64, lat: f64) -> (f64, f64) {
    // clamp latitude into Mercator’s valid range
    let clamped_lat = lat.clamp(-MAX_LAT, MAX_LAT);

    let x = lon * R_MAJOR * PI / 180.0;
    let lat_rad = clamped_lat * PI / 180.0;
    let y = R_MAJOR * ((PI / 4.0 + lat_rad / 2.0).tan().ln());
    (x, y)
}

/// from Web Mercator (x, y in meters) → longitude, latitude (degrees)
pub fn mercator_to_lon_lat(x: f64, y: f64) -> (f64, f64) {
    let lon = x / (R_MAJOR * PI / 180.0);
    let lat_rad = 2.0 * ((y / R_MAJOR).exp().atan()) - PI / 2.0;
    let lat = lat_rad * 180.0 / PI;
    (lon, lat)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proj::Proj;
    use rand::rngs::StdRng;
    use rand::{Rng, SeedableRng};

    const EPS: f64 = 1e-6;
    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < EPS
    }

    #[test]
    fn test_random_lon_lat_to_mercator_vs_proj() {
        let proj_merc = Proj::new_known_crs("EPSG:4326", "EPSG:3857", None).unwrap();
        let mut rng = StdRng::seed_from_u64(42);

        for _ in 0..1_000 {
            let lon = rng.random_range(-180.0..180.0);
            let lat = rng.random_range(-85.0..85.0);

            let (x1, y1) = lon_lat_to_mercator(lon, lat);
            let (x2, y2) = proj_merc.convert((lon, lat)).unwrap();

            assert!(approx_eq(x1, x2));
            assert!(approx_eq(y1, y2));
        }
    }

    #[test]
    fn test_lon_lat_to_mercator_clamps_lat_above_max() {
        let (x1, y1) = lon_lat_to_mercator(10.0, 90.0);
        let (x2, y2) = lon_lat_to_mercator(10.0, MAX_LAT);
        assert!(approx_eq(x1, x2));
        assert!(approx_eq(y1, y2));
    }

    #[test]
    fn test_lon_lat_to_mercator_clamps_lat_below_min() {
        let (x1, y1) = lon_lat_to_mercator(-20.0, -90.0);
        let (x2, y2) = lon_lat_to_mercator(-20.0, -MAX_LAT);
        assert!(approx_eq(x1, x2));
        assert!(approx_eq(y1, y2));
    }

    #[test]
    fn test_random_mercator_to_lon_lat_vs_proj() {
        let proj_geo = Proj::new_known_crs("EPSG:3857", "EPSG:4326", None).unwrap();
        let mut rng = StdRng::seed_from_u64(24);
        let bound = 20037508.342789244;

        for _ in 0..1_000 {
            let x = rng.random_range(-bound..bound);
            let y = rng.random_range(-bound..bound);
            let (lon1, lat1) = mercator_to_lon_lat(x, y);
            let (lon2, lat2) = proj_geo.convert((x, y)).unwrap();
            assert!(approx_eq(lon1, lon2));
            assert!(approx_eq(lat1, lat2));
        }
    }

    // ============================================================
    // NEW TESTS: CRS Transformer functionality
    // These tests ensure we correctly handle coordinate transformations
    // for multiple CRS types - the bug that was caught!
    // ============================================================

    #[test]
    fn test_create_transformer_3857_returns_none() {
        // When source is 3857, no transform is needed
        let result = create_transformer(3857);
        assert!(result.is_ok(), "Should succeed for EPSG:3857");
        assert!(result.unwrap().is_none(), "Should return None for same CRS (no transform needed)");
    }

    #[test]
    fn test_create_transformer_4326_returns_some() {
        // When source is 4326, we need a transformer
        let result = create_transformer(4326);
        assert!(result.is_ok(), "Should succeed for EPSG:4326");
        assert!(result.unwrap().is_some(), "Should return Some for different CRS");
    }

    #[test]
    fn test_create_transformer_utm_zone() {
        // Test UTM zone (common for many datasets)
        // EPSG:32633 is UTM zone 33N
        let result = create_transformer(32633);
        assert!(result.is_ok(), "Should succeed for UTM zone EPSG:32633");
        assert!(result.unwrap().is_some(), "Should return Some for UTM zone");
    }

    #[test]
    fn test_transform_coords_no_transform() {
        // When transformer is None, coords should pass through unchanged
        let transformer: Option<Proj> = None;
        let (x, y) = transform_coords(&transformer, 1000.0, 2000.0);
        assert_eq!(x, 1000.0, "X should be unchanged");
        assert_eq!(y, 2000.0, "Y should be unchanged");
    }

    #[test]
    fn test_transform_coords_3857_to_4326() {
        // Test actual coordinate transformation from 3857 to 4326
        let transformer = create_transformer(4326).unwrap();

        // Known point: Web Mercator (0, 0) = Geographic (0, 0)
        let (lon, lat) = transform_coords(&transformer, 0.0, 0.0);
        assert!(lon.abs() < 1e-6, "Longitude at origin should be ~0");
        assert!(lat.abs() < 1e-6, "Latitude at origin should be ~0");

        // Another known point: ~London in 3857
        // London is approximately at lon=-0.1, lat=51.5
        // In 3857: x=-11131.95, y=6711665.88
        let (lon2, lat2) = transform_coords(&transformer, -11131.95, 6711665.88);
        assert!((lon2 - (-0.1)).abs() < 0.01, "Longitude should be ~-0.1, got {}", lon2);
        assert!((lat2 - 51.5).abs() < 0.1, "Latitude should be ~51.5, got {}", lat2);
    }

    #[test]
    fn test_transform_coords_matches_manual_function() {
        // Verify that transform_coords with 4326 transformer gives same result
        // as our manual mercator_to_lon_lat function
        let transformer = create_transformer(4326).unwrap();

        let test_points = [
            (0.0, 0.0),
            (1000000.0, 1000000.0),
            (-5000000.0, 3000000.0),
            (10018754.17, 5000000.0),
        ];

        for (x, y) in test_points {
            let (lon1, lat1) = transform_coords(&transformer, x, y);
            let (lon2, lat2) = mercator_to_lon_lat(x, y);

            assert!(
                (lon1 - lon2).abs() < 1e-5,
                "Longitude mismatch at ({}, {}): proj={}, manual={}",
                x, y, lon1, lon2
            );
            assert!(
                (lat1 - lat2).abs() < 1e-5,
                "Latitude mismatch at ({}, {}): proj={}, manual={}",
                x, y, lat1, lat2
            );
        }
    }

    #[test]
    fn test_transform_handles_edge_coordinates() {
        // Test coordinates at the edge of Web Mercator extent
        let transformer = create_transformer(4326).unwrap();
        let max_extent = 20037508.342789244;

        // Near max extent
        let (lon, lat) = transform_coords(&transformer, max_extent * 0.99, max_extent * 0.5);
        assert!(lon.abs() < 180.0, "Longitude should be within ±180");
        assert!(lat.abs() < 90.0, "Latitude should be within ±90");
    }
}
