use std::f64::consts::PI;

/// WebMercator constants
const R_MAJOR: f64 = 6378137.0;

/// from longitude, latitude (degrees) → Web Mercator (x, y in meters)
pub fn lon_lat_to_mercator(lon: f64, lat: f64) -> (f64, f64) {
    let x = lon * R_MAJOR * PI / 180.0;
    let lat_rad = lat * PI / 180.0;
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

    // Generate 1000 uniformly random lon/lat pairs and 1000 random XYs within
    // Web Mercator’s bounds. To validate the internal conversion functions
    // against the more tested Proj library.
    #[test]
    fn test_random_lon_lat_to_mercator_vs_proj() {
        let proj_merc = Proj::new_known_crs("EPSG:4326", "EPSG:3857", None)
            .expect("failed to init proj 4326→3857");
        let mut rng = StdRng::seed_from_u64(42);

        for _ in 0..1_000 {
            // lon in [-180, 180], lat in [-85,85] for Mercator validity
            let lon = rng.random_range(-180.0..180.0);
            let lat = rng.random_range(-85.0..85.0);

            let (x1, y1) = lon_lat_to_mercator(lon, lat);
            let (x2, y2) = proj_merc.convert((lon, lat)).expect("proj convert failed");

            assert!(
                approx_eq(x1, x2),
                "x mismatch: {} vs {} at lon={}, lat={}",
                x1,
                x2,
                lon,
                lat
            );
            assert!(
                approx_eq(y1, y2),
                "y mismatch: {} vs {} at lon={}, lat={}",
                y1,
                y2,
                lon,
                lat
            );
        }
    }

    #[test]
    fn test_random_mercator_to_lon_lat_vs_proj() {
        let proj_geo = Proj::new_known_crs("EPSG:3857", "EPSG:4326", None)
            .expect("failed to init proj 3857→4326");
        let mut rng = StdRng::seed_from_u64(24);
        let bound = 20037508.342789244; // WebMercator world bounds

        for _ in 0..1_000 {
            let x = rng.random_range(-bound..bound);
            let y = rng.random_range(-bound..bound);

            let (lon1, lat1) = mercator_to_lon_lat(x, y);
            let (lon2, lat2) = proj_geo.convert((x, y)).expect("proj convert failed");

            assert!(
                approx_eq(lon1, lon2),
                "lon mismatch: {} vs {} at x={}, y={}",
                lon1,
                lon2,
                x,
                y
            );
            assert!(
                approx_eq(lat1, lat2),
                "lat mismatch: {} vs {} at x={}, y={}",
                lat1,
                lat2,
                x,
                y
            );
        }
    }
}
