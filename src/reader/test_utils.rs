//! Test utilities for generating synthetic raster data and validating visual correctness
//!
//! This module provides:
//! - Synthetic raster generators with known patterns
//! - Coordinate validation helpers
//! - Pixel value assertions

use crate::models::geometry::GeometryExtent;
use crate::reader::raster::RasterSource;
use std::sync::Arc;

/// A synthetic raster source for testing with predictable patterns
pub struct SyntheticRaster {
    width: usize,
    height: usize,
    bands: usize,
    pattern: RasterPattern,
    /// Optional transform: pixel scale and tiepoint
    pub pixel_scale: Option<[f64; 3]>,
    pub tiepoint: Option<[f64; 6]>,
}

/// Different patterns for synthetic test data
#[derive(Clone, Copy, Debug)]
pub enum RasterPattern {
    /// Value encodes position: value = x * 1000 + y
    /// Useful for validating coordinate transformations
    CoordinateEncoded,

    /// Horizontal gradient: value = x / (width - 1)
    /// Values range from 0.0 (left) to 1.0 (right)
    HorizontalGradient,

    /// Vertical gradient: value = y / (height - 1)
    /// Values range from 0.0 (top) to 1.0 (bottom)
    VerticalGradient,

    /// Checkerboard: alternating 0.0 and 1.0
    /// cell_size determines the size of each checker square
    Checkerboard { cell_size: usize },

    /// Constant value across entire raster
    Constant(f32),

    /// Ring pattern: value = distance from center (normalized)
    Ring,

    /// Quadrant pattern: different value in each quadrant
    /// Useful for verifying tile boundaries
    Quadrants,
}

impl SyntheticRaster {
    /// Create a new synthetic raster with the given dimensions and pattern
    pub fn new(width: usize, height: usize, pattern: RasterPattern) -> Self {
        Self {
            width,
            height,
            bands: 1,
            pattern,
            pixel_scale: None,
            tiepoint: None,
        }
    }

    /// Create with geographic transform (Web Mercator extent)
    pub fn with_web_mercator_extent(mut self, extent: &GeometryExtent) -> Self {
        let pixel_width = (extent.maxx - extent.minx) / self.width as f64;
        let pixel_height = (extent.maxy - extent.miny) / self.height as f64;

        self.pixel_scale = Some([pixel_width, pixel_height, 0.0]);
        self.tiepoint = Some([0.0, 0.0, 0.0, extent.minx, extent.maxy, 0.0]);
        self
    }

    /// Create with identity transform (pixel coords = world coords)
    pub fn with_identity_transform(mut self) -> Self {
        self.pixel_scale = Some([1.0, 1.0, 0.0]);
        self.tiepoint = Some([0.0, 0.0, 0.0, 0.0, self.height as f64, 0.0]);
        self
    }

    /// Get the expected value at a given pixel coordinate
    pub fn expected_value(&self, x: usize, y: usize) -> f32 {
        if x >= self.width || y >= self.height {
            return f32::NAN;
        }

        match self.pattern {
            RasterPattern::CoordinateEncoded => (x * 1000 + y) as f32,

            RasterPattern::HorizontalGradient => {
                if self.width <= 1 {
                    0.0
                } else {
                    x as f32 / (self.width - 1) as f32
                }
            }

            RasterPattern::VerticalGradient => {
                if self.height <= 1 {
                    0.0
                } else {
                    y as f32 / (self.height - 1) as f32
                }
            }

            RasterPattern::Checkerboard { cell_size } => {
                let cx = x / cell_size;
                let cy = y / cell_size;
                if (cx + cy) % 2 == 0 {
                    0.0
                } else {
                    1.0
                }
            }

            RasterPattern::Constant(v) => v,

            RasterPattern::Ring => {
                let cx = self.width as f64 / 2.0;
                let cy = self.height as f64 / 2.0;
                let dx = x as f64 - cx;
                let dy = y as f64 - cy;
                let dist = (dx * dx + dy * dy).sqrt();
                let max_dist = (cx * cx + cy * cy).sqrt();
                (dist / max_dist) as f32
            }

            RasterPattern::Quadrants => {
                let in_left = x < self.width / 2;
                let in_top = y < self.height / 2;
                match (in_left, in_top) {
                    (true, true) => 1.0,   // Top-left
                    (false, true) => 2.0,  // Top-right
                    (true, false) => 3.0,  // Bottom-left
                    (false, false) => 4.0, // Bottom-right
                }
            }
        }
    }

    /// Decode position from a coordinate-encoded value
    pub fn decode_position(value: f32) -> Option<(usize, usize)> {
        if value.is_nan() || value < 0.0 {
            return None;
        }
        let v = value as usize;
        let x = v / 1000;
        let y = v % 1000;
        Some((x, y))
    }

    /// Get extent in pixel coordinates
    pub fn pixel_extent(&self) -> GeometryExtent {
        GeometryExtent {
            minx: 0.0,
            miny: 0.0,
            maxx: self.width as f64,
            maxy: self.height as f64,
        }
    }

    /// Get extent in world coordinates (if transform is set)
    pub fn world_extent(&self) -> Option<GeometryExtent> {
        let scale = self.pixel_scale?;
        let tie = self.tiepoint?;

        Some(GeometryExtent {
            minx: tie[3],
            maxy: tie[4],
            maxx: tie[3] + self.width as f64 * scale[0],
            miny: tie[4] - self.height as f64 * scale[1],
        })
    }

    /// Generate all pixel values as a flat Vec
    pub fn to_vec(&self) -> Vec<f32> {
        let mut data = Vec::with_capacity(self.width * self.height * self.bands);
        for y in 0..self.height {
            for x in 0..self.width {
                for _b in 0..self.bands {
                    data.push(self.expected_value(x, y));
                }
            }
        }
        data
    }
}

impl RasterSource for SyntheticRaster {
    fn bands(&self) -> usize {
        self.bands
    }

    fn width(&self) -> usize {
        self.width
    }

    fn height(&self) -> usize {
        self.height
    }

    fn sample(&self, band: usize, x: usize, y: usize) -> Option<f32> {
        if band >= self.bands || x >= self.width || y >= self.height {
            return None;
        }
        Some(self.expected_value(x, y))
    }
}

/// Assert that two f32 values are approximately equal
pub fn assert_approx_eq(actual: f32, expected: f32, epsilon: f32, context: &str) {
    if expected.is_nan() {
        assert!(actual.is_nan(), "{}: expected NaN, got {}", context, actual);
    } else {
        assert!(
            (actual - expected).abs() <= epsilon,
            "{}: expected {}, got {} (diff: {})",
            context,
            expected,
            actual,
            (actual - expected).abs()
        );
    }
}

/// Assert that a pixel value matches the expected synthetic value
pub fn assert_pixel_matches(
    source: &SyntheticRaster,
    actual: f32,
    x: usize,
    y: usize,
    epsilon: f32,
) {
    let expected = source.expected_value(x, y);
    assert_approx_eq(
        actual,
        expected,
        epsilon,
        &format!("pixel ({}, {})", x, y),
    );
}

/// Validate that extracted tile data matches the synthetic source
pub fn validate_tile_extraction(
    source: &SyntheticRaster,
    extracted_data: &[f32],
    tile_extent: &GeometryExtent,
    tile_width: usize,
    tile_height: usize,
    epsilon: f32,
) -> Result<(), String> {
    if extracted_data.len() != tile_width * tile_height {
        return Err(format!(
            "Tile data length mismatch: expected {}, got {}",
            tile_width * tile_height,
            extracted_data.len()
        ));
    }

    let scale = source.pixel_scale.ok_or("No pixel scale set")?;
    let tie = source.tiepoint.ok_or("No tiepoint set")?;

    let res_x = (tile_extent.maxx - tile_extent.minx) / tile_width as f64;
    let res_y = (tile_extent.maxy - tile_extent.miny) / tile_height as f64;

    let mut errors = Vec::new();

    for ty in 0..tile_height {
        for tx in 0..tile_width {
            let world_x = tile_extent.minx + (tx as f64 + 0.5) * res_x;
            let world_y = tile_extent.maxy - (ty as f64 + 0.5) * res_y;

            // Convert world to pixel
            let px = ((world_x - tie[3]) / scale[0]) as usize;
            let py = ((tie[4] - world_y) / scale[1]) as usize;

            let actual = extracted_data[ty * tile_width + tx];
            let expected = source.expected_value(px, py);

            if expected.is_nan() {
                if !actual.is_nan() {
                    errors.push(format!(
                        "Tile ({}, {}) -> Source ({}, {}): expected NaN, got {}",
                        tx, ty, px, py, actual
                    ));
                }
            } else if (actual - expected).abs() > epsilon {
                errors.push(format!(
                    "Tile ({}, {}) -> Source ({}, {}): expected {}, got {}",
                    tx, ty, px, py, expected, actual
                ));
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!("{} pixel mismatches:\n{}", errors.len(), errors.join("\n")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_coordinate_encoded_pattern() {
        let raster = SyntheticRaster::new(100, 100, RasterPattern::CoordinateEncoded);

        assert_eq!(raster.expected_value(0, 0), 0.0);
        assert_eq!(raster.expected_value(1, 0), 1000.0);
        assert_eq!(raster.expected_value(0, 1), 1.0);
        assert_eq!(raster.expected_value(5, 7), 5007.0);
        assert_eq!(raster.expected_value(99, 99), 99099.0);
    }

    #[test]
    fn test_decode_position() {
        assert_eq!(SyntheticRaster::decode_position(0.0), Some((0, 0)));
        assert_eq!(SyntheticRaster::decode_position(5007.0), Some((5, 7)));
        assert_eq!(SyntheticRaster::decode_position(99099.0), Some((99, 99)));
        assert_eq!(SyntheticRaster::decode_position(f32::NAN), None);
    }

    #[test]
    fn test_horizontal_gradient() {
        let raster = SyntheticRaster::new(11, 5, RasterPattern::HorizontalGradient);

        assert_approx_eq(raster.expected_value(0, 0), 0.0, 0.001, "left edge");
        assert_approx_eq(raster.expected_value(5, 0), 0.5, 0.001, "middle");
        assert_approx_eq(raster.expected_value(10, 0), 1.0, 0.001, "right edge");

        // Same across all rows
        assert_approx_eq(raster.expected_value(5, 0), raster.expected_value(5, 4), 0.001, "same value across rows");
    }

    #[test]
    fn test_vertical_gradient() {
        let raster = SyntheticRaster::new(5, 11, RasterPattern::VerticalGradient);

        assert_approx_eq(raster.expected_value(0, 0), 0.0, 0.001, "top edge");
        assert_approx_eq(raster.expected_value(0, 5), 0.5, 0.001, "middle");
        assert_approx_eq(raster.expected_value(0, 10), 1.0, 0.001, "bottom edge");
    }

    #[test]
    fn test_checkerboard() {
        let raster = SyntheticRaster::new(10, 10, RasterPattern::Checkerboard { cell_size: 2 });

        // Cell (0,0) - top-left 2x2 block
        assert_eq!(raster.expected_value(0, 0), 0.0);
        assert_eq!(raster.expected_value(1, 1), 0.0);

        // Cell (1,0) - second block in first row
        assert_eq!(raster.expected_value(2, 0), 1.0);
        assert_eq!(raster.expected_value(3, 1), 1.0);

        // Cell (0,1) - first block in second row
        assert_eq!(raster.expected_value(0, 2), 1.0);
        assert_eq!(raster.expected_value(1, 3), 1.0);
    }

    #[test]
    fn test_quadrants() {
        let raster = SyntheticRaster::new(100, 100, RasterPattern::Quadrants);

        // Top-left
        assert_eq!(raster.expected_value(0, 0), 1.0);
        assert_eq!(raster.expected_value(49, 49), 1.0);

        // Top-right
        assert_eq!(raster.expected_value(50, 0), 2.0);
        assert_eq!(raster.expected_value(99, 49), 2.0);

        // Bottom-left
        assert_eq!(raster.expected_value(0, 50), 3.0);
        assert_eq!(raster.expected_value(49, 99), 3.0);

        // Bottom-right
        assert_eq!(raster.expected_value(50, 50), 4.0);
        assert_eq!(raster.expected_value(99, 99), 4.0);
    }

    #[test]
    fn test_raster_source_impl() {
        let raster = SyntheticRaster::new(50, 30, RasterPattern::Constant(42.0));

        assert_eq!(raster.width(), 50);
        assert_eq!(raster.height(), 30);
        assert_eq!(raster.bands(), 1);
        assert_eq!(raster.sample(0, 25, 15), Some(42.0));
        assert_eq!(raster.sample(0, 100, 0), None); // Out of bounds
        assert_eq!(raster.sample(1, 0, 0), None); // Invalid band
    }

    #[test]
    fn test_with_identity_transform() {
        let raster = SyntheticRaster::new(100, 100, RasterPattern::Constant(1.0))
            .with_identity_transform();

        assert_eq!(raster.pixel_scale, Some([1.0, 1.0, 0.0]));
        assert_eq!(raster.tiepoint, Some([0.0, 0.0, 0.0, 0.0, 100.0, 0.0]));

        let extent = raster.world_extent().unwrap();
        assert_eq!(extent.minx, 0.0);
        assert_eq!(extent.maxy, 100.0);
        assert_eq!(extent.maxx, 100.0);
        assert_eq!(extent.miny, 0.0);
    }

    #[test]
    fn test_with_web_mercator_extent() {
        let extent = GeometryExtent::new(-1000.0, -1000.0, 1000.0, 1000.0);
        let raster = SyntheticRaster::new(100, 100, RasterPattern::Constant(1.0))
            .with_web_mercator_extent(&extent);

        let scale = raster.pixel_scale.unwrap();
        assert_approx_eq(scale[0] as f32, 20.0, 0.001, "pixel width");
        assert_approx_eq(scale[1] as f32, 20.0, 0.001, "pixel height");

        let tie = raster.tiepoint.unwrap();
        assert_eq!(tie[3], -1000.0); // origin X
        assert_eq!(tie[4], 1000.0);  // origin Y (top)
    }

    #[test]
    fn test_to_vec() {
        let raster = SyntheticRaster::new(3, 2, RasterPattern::CoordinateEncoded);
        let data = raster.to_vec();

        assert_eq!(data.len(), 6);
        // Row 0: (0,0), (1,0), (2,0)
        assert_eq!(data[0], 0.0);    // x=0, y=0
        assert_eq!(data[1], 1000.0); // x=1, y=0
        assert_eq!(data[2], 2000.0); // x=2, y=0
        // Row 1: (0,1), (1,1), (2,1)
        assert_eq!(data[3], 1.0);    // x=0, y=1
        assert_eq!(data[4], 1001.0); // x=1, y=1
        assert_eq!(data[5], 2001.0); // x=2, y=1
    }
}
