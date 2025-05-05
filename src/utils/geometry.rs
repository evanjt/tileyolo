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
