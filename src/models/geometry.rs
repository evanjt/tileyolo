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
        (f64::midpoint(self.minx, self.maxx), f64::midpoint(self.miny, self.maxy))
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
    fn test_intersects_edge_cases() {
        let a = GeometryExtent::new(0.0, 0.0, 10.0, 10.0);

        // Adjacent extents (sharing an edge) should intersect
        let adjacent_right = GeometryExtent::new(10.0, 0.0, 20.0, 10.0);
        assert!(a.intersects(&adjacent_right));

        // Adjacent extents (sharing a corner) should intersect
        let corner = GeometryExtent::new(10.0, 10.0, 20.0, 20.0);
        assert!(a.intersects(&corner));

        // Completely contained should intersect
        let inside = GeometryExtent::new(2.0, 2.0, 8.0, 8.0);
        assert!(a.intersects(&inside));
        assert!(inside.intersects(&a));
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
    fn test_intersection_no_overlap() {
        let a = GeometryExtent::new(0.0, 0.0, 10.0, 10.0);
        let c = GeometryExtent::new(20.0, 20.0, 30.0, 30.0);

        assert!(a.intersection(&c).is_none());
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

    #[test]
    fn test_contains_extent() {
        let outer = GeometryExtent::new(0.0, 0.0, 100.0, 100.0);
        let inner = GeometryExtent::new(10.0, 10.0, 50.0, 50.0);
        let partial = GeometryExtent::new(50.0, 50.0, 150.0, 150.0);
        let outside = GeometryExtent::new(200.0, 200.0, 300.0, 300.0);

        assert!(outer.contains(&inner));
        assert!(!inner.contains(&outer));
        assert!(!outer.contains(&partial));
        assert!(!outer.contains(&outside));

        // Extent contains itself
        assert!(outer.contains(&outer));
    }

    #[test]
    fn test_width_height() {
        let extent = GeometryExtent::new(10.0, 20.0, 50.0, 80.0);

        assert!((extent.width() - 40.0).abs() < 0.001);
        assert!((extent.height() - 60.0).abs() < 0.001);
    }

    #[test]
    fn test_area() {
        let extent = GeometryExtent::new(0.0, 0.0, 10.0, 20.0);
        assert!((extent.area() - 200.0).abs() < 0.001);

        let small = GeometryExtent::new(0.0, 0.0, 1.0, 1.0);
        assert!((small.area() - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_center() {
        let extent = GeometryExtent::new(0.0, 0.0, 100.0, 100.0);
        let (cx, cy) = extent.center();
        assert!((cx - 50.0).abs() < 0.001);
        assert!((cy - 50.0).abs() < 0.001);

        let offset = GeometryExtent::new(10.0, 20.0, 30.0, 40.0);
        let (ox, oy) = offset.center();
        assert!((ox - 20.0).abs() < 0.001);
        assert!((oy - 30.0).abs() < 0.001);
    }

    #[test]
    fn test_is_valid() {
        let valid = GeometryExtent::new(0.0, 0.0, 10.0, 10.0);
        assert!(valid.is_valid());

        // Zero-width is invalid
        let zero_width = GeometryExtent::new(5.0, 0.0, 5.0, 10.0);
        assert!(!zero_width.is_valid());

        // Zero-height is invalid
        let zero_height = GeometryExtent::new(0.0, 5.0, 10.0, 5.0);
        assert!(!zero_height.is_valid());

        // Inverted extent is invalid
        let inverted = GeometryExtent::new(10.0, 10.0, 0.0, 0.0);
        assert!(!inverted.is_valid());
    }

    #[test]
    fn test_buffer() {
        let extent = GeometryExtent::new(10.0, 10.0, 20.0, 20.0);
        let buffered = extent.buffer(5.0);

        assert!((buffered.minx - 5.0).abs() < 0.001);
        assert!((buffered.miny - 5.0).abs() < 0.001);
        assert!((buffered.maxx - 25.0).abs() < 0.001);
        assert!((buffered.maxy - 25.0).abs() < 0.001);

        // Negative buffer (shrink)
        let shrunk = extent.buffer(-2.0);
        assert!((shrunk.minx - 12.0).abs() < 0.001);
        assert!((shrunk.miny - 12.0).abs() < 0.001);
        assert!((shrunk.maxx - 18.0).abs() < 0.001);
        assert!((shrunk.maxy - 18.0).abs() < 0.001);
    }

    #[test]
    fn test_from_tuple() {
        let extent: GeometryExtent = (1.0, 2.0, 3.0, 4.0).into();
        assert!((extent.minx - 1.0).abs() < 0.001);
        assert!((extent.miny - 2.0).abs() < 0.001);
        assert!((extent.maxx - 3.0).abs() < 0.001);
        assert!((extent.maxy - 4.0).abs() < 0.001);
    }

    #[test]
    fn test_default() {
        let extent = GeometryExtent::default();
        assert!((extent.minx - 0.0).abs() < 0.001);
        assert!((extent.miny - 0.0).abs() < 0.001);
        assert!((extent.maxx - 0.0).abs() < 0.001);
        assert!((extent.maxy - 0.0).abs() < 0.001);
    }
}
