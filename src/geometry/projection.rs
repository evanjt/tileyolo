use std::f64::consts::PI;

/// WebMercator constants
const R_MAJOR: f64 = 6378137.0;
const MAX_LAT: f64 = 85.05112877980659; // Max bounds for Web Mercator

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
}
