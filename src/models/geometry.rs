use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GeometryExtent {
    pub minx: f64,
    pub miny: f64,
    pub maxx: f64,
    pub maxy: f64,
}

impl GeometryExtent {
    /// Create a new extent from min/max coordinates
    pub fn new(minx: f64, miny: f64, maxx: f64, maxy: f64) -> Self {
        Self { minx, miny, maxx, maxy }
    }

    /// Check if this extent intersects with another
    #[inline]
    pub fn intersects(&self, other: &GeometryExtent) -> bool {
        self.minx <= other.maxx
            && self.maxx >= other.minx
            && self.miny <= other.maxy
            && self.maxy >= other.miny
    }

    /// Check if this extent contains a point
    #[inline]
    pub fn contains_point(&self, x: f64, y: f64) -> bool {
        x >= self.minx && x <= self.maxx && y >= self.miny && y <= self.maxy
    }

    /// Check if this extent fully contains another extent
    #[inline]
    pub fn contains(&self, other: &GeometryExtent) -> bool {
        self.minx <= other.minx
            && self.maxx >= other.maxx
            && self.miny <= other.miny
            && self.maxy >= other.maxy
    }

    /// Compute the intersection of two extents (if they intersect)
    pub fn intersection(&self, other: &GeometryExtent) -> Option<GeometryExtent> {
        if !self.intersects(other) {
            return None;
        }

        Some(GeometryExtent {
            minx: self.minx.max(other.minx),
            miny: self.miny.max(other.miny),
            maxx: self.maxx.min(other.maxx),
            maxy: self.maxy.min(other.maxy),
        })
    }

    /// Get the width of the extent
    #[inline]
    pub fn width(&self) -> f64 {
        self.maxx - self.minx
    }

    /// Get the height of the extent
    #[inline]
    pub fn height(&self) -> f64 {
        self.maxy - self.miny
    }

    /// Get the area of the extent
    #[inline]
    pub fn area(&self) -> f64 {
        self.width() * self.height()
    }

    /// Get the center point of the extent
    #[inline]
    pub fn center(&self) -> (f64, f64) {
        ((self.minx + self.maxx) / 2.0, (self.miny + self.maxy) / 2.0)
    }

    /// Check if the extent is valid (non-empty, non-inverted)
    #[inline]
    pub fn is_valid(&self) -> bool {
        self.maxx > self.minx && self.maxy > self.miny
    }

    /// Expand extent by a buffer distance on all sides
    pub fn buffer(&self, distance: f64) -> Self {
        Self {
            minx: self.minx - distance,
            miny: self.miny - distance,
            maxx: self.maxx + distance,
            maxy: self.maxy + distance,
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_intersects() {
        let a = GeometryExtent::new(0.0, 0.0, 10.0, 10.0);
        let b = GeometryExtent::new(5.0, 5.0, 15.0, 15.0);
        let c = GeometryExtent::new(20.0, 20.0, 30.0, 30.0);

        assert!(a.intersects(&b));
        assert!(b.intersects(&a));
        assert!(!a.intersects(&c));
        assert!(!c.intersects(&a));
    }

    #[test]
    fn test_intersection() {
        let a = GeometryExtent::new(0.0, 0.0, 10.0, 10.0);
        let b = GeometryExtent::new(5.0, 5.0, 15.0, 15.0);

        let inter = a.intersection(&b).unwrap();
        assert!((inter.minx - 5.0).abs() < 0.001);
        assert!((inter.miny - 5.0).abs() < 0.001);
        assert!((inter.maxx - 10.0).abs() < 0.001);
        assert!((inter.maxy - 10.0).abs() < 0.001);
    }

    #[test]
    fn test_contains_point() {
        let extent = GeometryExtent::new(0.0, 0.0, 10.0, 10.0);

        assert!(extent.contains_point(5.0, 5.0));
        assert!(extent.contains_point(0.0, 0.0));
        assert!(extent.contains_point(10.0, 10.0));
        assert!(!extent.contains_point(-1.0, 5.0));
        assert!(!extent.contains_point(11.0, 5.0));
    }
}
