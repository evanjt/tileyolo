use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct GeometryExtent {
    pub minx: f64,
    pub miny: f64,
    pub maxx: f64,
    pub maxy: f64,
}

impl From<(f64, f64, f64, f64)> for GeometryExtent {
    fn from(extent: (f64, f64, f64, f64)) -> Self {
        GeometryExtent {
            minx: extent.0, // minx
            miny: extent.1, // miny
            maxx: extent.2, // maxx
            maxy: extent.3, // maxy
        }
    }
}
