//! Pure Rust test COG generator with deterministic patterns.
//!
//! Generates Cloud Optimized GeoTIFFs with predictable pixel values that can be
//! used as ground truth for testing tile readers. The pattern encodes position
//! information directly into pixel values, enabling verification of coordinate
//! transformations and data extraction.
//!
//! # Pattern Design
//!
//! The default pattern encodes X and Y position into the pixel value:
//! - For 8-bit: `value = ((x % 16) << 4) | (y % 16)` giving a 16x16 repeating grid
//! - This creates a checkerboard-like pattern where each 16x16 block is unique
//! - Pixel value can be decoded back to relative position within the block
//!
//! # Usage
//!
//! ```rust,ignore
//! use tileyolo::test_cog_generator::{TestCogGenerator, TestPattern};
//!
//! let generator = TestCogGenerator::new()
//!     .size(512, 512)
//!     .epsg(3857)
//!     .world_bounds()
//!     .pattern(TestPattern::PositionEncoded);
//!
//! // Generate raw pixel data
//! let pixels = generator.generate_pixels();
//!
//! // Write to TIFF file
//! generator.write_tiff("test.tif")?;
//!
//! // Verify a pixel value
//! let expected = generator.expected_value(100, 200);
//! ```

use std::io::{Seek, Write};
use std::path::Path;

/// Pattern types for test image generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestPattern {
    /// Encodes X/Y position: `((x % 16) << 4) | (y % 16)`
    /// Creates a 16x16 repeating pattern where each cell has a unique value 0-255
    PositionEncoded,

    /// Checkerboard pattern with configurable cell size
    /// Even cells = 50, Odd cells = 200
    Checkerboard { cell_size: u32 },

    /// Horizontal gradient from 0 to 255
    HorizontalGradient,

    /// Vertical gradient from 0 to 255
    VerticalGradient,

    /// Diagonal gradient (x + y) % 256
    DiagonalGradient,

    /// Concentric rings from center
    ConcentricRings { ring_width: u32 },

    /// Solid color (for edge case testing)
    Solid { value: u8 },
}

impl Default for TestPattern {
    fn default() -> Self {
        Self::PositionEncoded
    }
}

/// Configuration for test COG generation.
#[derive(Debug, Clone)]
pub struct TestCogGenerator {
    width: u32,
    height: u32,
    tile_size: u32,
    epsg: u32,
    bounds: (f64, f64, f64, f64), // minx, miny, maxx, maxy
    pattern: TestPattern,
    nodata: Option<u8>,
    compression: Compression,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    None,
    Deflate,
}

impl Default for TestCogGenerator {
    fn default() -> Self {
        Self::new()
    }
}

impl TestCogGenerator {
    /// Create a new generator with default settings.
    ///
    /// Defaults:
    /// - Size: 512x512
    /// - Tile size: 256
    /// - EPSG: 3857 (Web Mercator)
    /// - Bounds: World extent in Web Mercator
    /// - Pattern: PositionEncoded
    pub fn new() -> Self {
        Self {
            width: 512,
            height: 512,
            tile_size: 256,
            epsg: 3857,
            bounds: (
                -20037508.342789244,
                -20037508.342789244,
                20037508.342789244,
                20037508.342789244,
            ),
            pattern: TestPattern::PositionEncoded,
            nodata: Some(0),
            compression: Compression::Deflate,
        }
    }

    /// Set the image dimensions.
    pub fn size(mut self, width: u32, height: u32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    /// Set the internal tile size (must be power of 2, typically 256 or 512).
    pub fn tile_size(mut self, size: u32) -> Self {
        self.tile_size = size;
        self
    }

    /// Set the EPSG code for the coordinate reference system.
    pub fn epsg(mut self, code: u32) -> Self {
        self.epsg = code;
        self
    }

    /// Set geographic bounds (minx, miny, maxx, maxy).
    pub fn bounds(mut self, minx: f64, miny: f64, maxx: f64, maxy: f64) -> Self {
        self.bounds = (minx, miny, maxx, maxy);
        self
    }

    /// Use world bounds for Web Mercator (EPSG:3857).
    pub fn world_bounds(mut self) -> Self {
        self.bounds = (
            -20037508.342789244,
            -20037508.342789244,
            20037508.342789244,
            20037508.342789244,
        );
        self.epsg = 3857;
        self
    }

    /// Set the pixel pattern.
    pub fn pattern(mut self, pattern: TestPattern) -> Self {
        self.pattern = pattern;
        self
    }

    /// Set the NoData value (None to disable).
    pub fn nodata(mut self, value: Option<u8>) -> Self {
        self.nodata = value;
        self
    }

    /// Set compression method.
    pub fn compression(mut self, compression: Compression) -> Self {
        self.compression = compression;
        self
    }

    /// Calculate the expected pixel value at a given position.
    ///
    /// This is the ground truth function - use it to verify tile reader output.
    pub fn expected_value(&self, x: u32, y: u32) -> u8 {
        if x >= self.width || y >= self.height {
            return self.nodata.unwrap_or(0);
        }

        match self.pattern {
            TestPattern::PositionEncoded => {
                // Encode position: high nibble = x % 16, low nibble = y % 16
                (((x % 16) << 4) | (y % 16)) as u8
            }
            TestPattern::Checkerboard { cell_size } => {
                let cell_x = x / cell_size;
                let cell_y = y / cell_size;
                if (cell_x + cell_y) % 2 == 0 {
                    50
                } else {
                    200
                }
            }
            TestPattern::HorizontalGradient => {
                ((x as f64 / self.width as f64) * 255.0) as u8
            }
            TestPattern::VerticalGradient => {
                ((y as f64 / self.height as f64) * 255.0) as u8
            }
            TestPattern::DiagonalGradient => ((x + y) % 256) as u8,
            TestPattern::ConcentricRings { ring_width } => {
                let cx = self.width as f64 / 2.0;
                let cy = self.height as f64 / 2.0;
                let dx = x as f64 - cx;
                let dy = y as f64 - cy;
                let dist = (dx * dx + dy * dy).sqrt();
                let ring = (dist / ring_width as f64) as u32;
                if ring % 2 == 0 {
                    50
                } else {
                    200
                }
            }
            TestPattern::Solid { value } => value,
        }
    }

    /// Decode a position-encoded pixel value back to relative coordinates.
    ///
    /// Only valid for `TestPattern::PositionEncoded`.
    /// Returns (x % 16, y % 16).
    pub fn decode_position(value: u8) -> (u8, u8) {
        let x = (value >> 4) & 0x0F;
        let y = value & 0x0F;
        (x, y)
    }

    /// Convert pixel coordinates to geographic coordinates.
    pub fn pixel_to_geo(&self, x: u32, y: u32) -> (f64, f64) {
        let (minx, miny, maxx, maxy) = self.bounds;
        let pixel_width = (maxx - minx) / self.width as f64;
        let pixel_height = (maxy - miny) / self.height as f64;

        // Upper-left origin, Y increases downward in pixel space
        let geo_x = minx + (x as f64 + 0.5) * pixel_width;
        let geo_y = maxy - (y as f64 + 0.5) * pixel_height;

        (geo_x, geo_y)
    }

    /// Convert geographic coordinates to pixel coordinates.
    pub fn geo_to_pixel(&self, geo_x: f64, geo_y: f64) -> (u32, u32) {
        let (minx, miny, maxx, maxy) = self.bounds;
        let pixel_width = (maxx - minx) / self.width as f64;
        let pixel_height = (maxy - miny) / self.height as f64;

        let x = ((geo_x - minx) / pixel_width) as u32;
        let y = ((maxy - geo_y) / pixel_height) as u32;

        (x.min(self.width - 1), y.min(self.height - 1))
    }

    /// Generate raw pixel data as a Vec<u8>.
    pub fn generate_pixels(&self) -> Vec<u8> {
        let mut pixels = Vec::with_capacity((self.width * self.height) as usize);

        for y in 0..self.height {
            for x in 0..self.width {
                pixels.push(self.expected_value(x, y));
            }
        }

        pixels
    }

    /// Write a minimal valid TIFF file (not a full COG, but readable).
    ///
    /// For CI testing, this creates a basic GeoTIFF that our readers can parse.
    pub fn write_tiff<P: AsRef<Path>>(&self, path: P) -> std::io::Result<()> {
        let pixels = self.generate_pixels();
        let mut file = std::fs::File::create(path)?;

        // Calculate layout:
        // Header: 8 bytes
        // IFD entry count: 2 bytes
        // IFD entries: 13 * 12 = 156 bytes
        // Next IFD pointer: 4 bytes
        // ModelPixelScale: 24 bytes (3 doubles)
        // ModelTiepoint: 48 bytes (6 doubles)
        // GeoKeyDirectory: 32 bytes (16 shorts)
        // Total before pixel data: 8 + 2 + 156 + 4 + 24 + 48 + 32 = 274 bytes

        let num_entries: u16 = 13;
        let ifd_offset: u32 = 8;
        let ifd_entries_size = num_entries as u32 * 12;
        let after_ifd = ifd_offset + 2 + ifd_entries_size + 4;
        let pixel_scale_offset = after_ifd;
        let tiepoint_offset = pixel_scale_offset + 24;
        let geo_key_offset = tiepoint_offset + 48;
        let image_data_offset = geo_key_offset + 32;

        // TIFF Header (8 bytes)
        file.write_all(b"II")?; // Little-endian
        file.write_all(&42u16.to_le_bytes())?; // TIFF magic
        file.write_all(&ifd_offset.to_le_bytes())?; // IFD offset

        // IFD (Image File Directory)
        file.write_all(&num_entries.to_le_bytes())?;

        // Tag entries (12 bytes each): tag(2) + type(2) + count(4) + value/offset(4)
        // 256: ImageWidth
        write_ifd_entry(&mut file, 256, 3, 1, self.width)?;
        // 257: ImageLength
        write_ifd_entry(&mut file, 257, 3, 1, self.height)?;
        // 258: BitsPerSample
        write_ifd_entry(&mut file, 258, 3, 1, 8)?;
        // 259: Compression (1 = None)
        write_ifd_entry(&mut file, 259, 3, 1, 1)?;
        // 262: PhotometricInterpretation (1 = MinIsBlack)
        write_ifd_entry(&mut file, 262, 3, 1, 1)?;
        // 273: StripOffsets
        write_ifd_entry(&mut file, 273, 4, 1, image_data_offset)?;
        // 277: SamplesPerPixel
        write_ifd_entry(&mut file, 277, 3, 1, 1)?;
        // 278: RowsPerStrip
        write_ifd_entry(&mut file, 278, 3, 1, self.height)?;
        // 279: StripByteCounts
        write_ifd_entry(&mut file, 279, 4, 1, self.width * self.height)?;
        // 284: PlanarConfiguration (1 = Chunky)
        write_ifd_entry(&mut file, 284, 3, 1, 1)?;
        // 33550: ModelPixelScaleTag (3 doubles)
        write_ifd_entry(&mut file, 33550, 12, 3, pixel_scale_offset)?;
        // 33922: ModelTiepointTag (6 doubles)
        write_ifd_entry(&mut file, 33922, 12, 6, tiepoint_offset)?;
        // 34735: GeoKeyDirectoryTag
        write_ifd_entry(&mut file, 34735, 3, 16, geo_key_offset)?;

        // Next IFD offset (0 = none)
        file.write_all(&0u32.to_le_bytes())?;

        // ModelPixelScaleTag (3 doubles = 24 bytes)
        let (minx, miny, maxx, maxy) = self.bounds;
        let pixel_width = (maxx - minx) / self.width as f64;
        let pixel_height = (maxy - miny) / self.height as f64;
        file.write_all(&pixel_width.to_le_bytes())?;
        file.write_all(&pixel_height.to_le_bytes())?;
        file.write_all(&0.0f64.to_le_bytes())?;

        // ModelTiepointTag (6 doubles = 48 bytes)
        // Maps pixel (0,0) to geographic (minx, maxy)
        file.write_all(&0.0f64.to_le_bytes())?; // I (pixel X)
        file.write_all(&0.0f64.to_le_bytes())?; // J (pixel Y)
        file.write_all(&0.0f64.to_le_bytes())?; // K (pixel Z)
        file.write_all(&minx.to_le_bytes())?; // X
        file.write_all(&maxy.to_le_bytes())?; // Y
        file.write_all(&0.0f64.to_le_bytes())?; // Z

        // GeoKeyDirectoryTag (16 shorts = 32 bytes)
        // Version, revision, minor, number of keys
        file.write_all(&1u16.to_le_bytes())?; // KeyDirectoryVersion
        file.write_all(&1u16.to_le_bytes())?; // KeyRevision
        file.write_all(&0u16.to_le_bytes())?; // MinorRevision
        file.write_all(&3u16.to_le_bytes())?; // NumberOfKeys

        // Key 1: GTModelTypeGeoKey = 1 (Projected)
        file.write_all(&1024u16.to_le_bytes())?;
        file.write_all(&0u16.to_le_bytes())?;
        file.write_all(&1u16.to_le_bytes())?;
        file.write_all(&1u16.to_le_bytes())?;

        // Key 2: GTRasterTypeGeoKey = 1 (PixelIsArea)
        file.write_all(&1025u16.to_le_bytes())?;
        file.write_all(&0u16.to_le_bytes())?;
        file.write_all(&1u16.to_le_bytes())?;
        file.write_all(&1u16.to_le_bytes())?;

        // Key 3: ProjectedCSTypeGeoKey = EPSG code
        file.write_all(&3072u16.to_le_bytes())?;
        file.write_all(&0u16.to_le_bytes())?;
        file.write_all(&1u16.to_le_bytes())?;
        file.write_all(&(self.epsg as u16).to_le_bytes())?;

        // Verify we're at the expected position
        let current_pos = file.stream_position()? as u32;
        assert_eq!(current_pos, image_data_offset,
            "TIFF layout error: expected to be at {}, but at {}", image_data_offset, current_pos);

        // Write pixel data
        file.write_all(&pixels)?;

        Ok(())
    }

    /// Get image dimensions.
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Get the bounds.
    pub fn get_bounds(&self) -> (f64, f64, f64, f64) {
        self.bounds
    }
}

fn write_ifd_entry<W: Write>(
    writer: &mut W,
    tag: u16,
    field_type: u16,
    count: u32,
    value: u32,
) -> std::io::Result<()> {
    writer.write_all(&tag.to_le_bytes())?;
    writer.write_all(&field_type.to_le_bytes())?;
    writer.write_all(&count.to_le_bytes())?;
    writer.write_all(&value.to_le_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_position_encoded_pattern() {
        let cog = TestCogGenerator::new().pattern(TestPattern::PositionEncoded);

        // Test some known positions
        assert_eq!(cog.expected_value(0, 0), 0b0000_0000); // (0,0) -> 0
        assert_eq!(cog.expected_value(1, 0), 0b0001_0000); // (1,0) -> 16
        assert_eq!(cog.expected_value(0, 1), 0b0000_0001); // (0,1) -> 1
        assert_eq!(cog.expected_value(15, 15), 0b1111_1111); // (15,15) -> 255
        assert_eq!(cog.expected_value(16, 16), 0b0000_0000); // wraps to (0,0)
    }

    #[test]
    fn test_decode_position() {
        assert_eq!(TestCogGenerator::decode_position(0b0000_0000), (0, 0));
        assert_eq!(TestCogGenerator::decode_position(0b0001_0000), (1, 0));
        assert_eq!(TestCogGenerator::decode_position(0b0000_0001), (0, 1));
        assert_eq!(TestCogGenerator::decode_position(0b1111_1111), (15, 15));
        assert_eq!(TestCogGenerator::decode_position(0b1010_0101), (10, 5));
    }

    #[test]
    fn test_checkerboard_pattern() {
        let cog = TestCogGenerator::new().pattern(TestPattern::Checkerboard { cell_size: 16 });

        // Cell (0,0) should be 50
        assert_eq!(cog.expected_value(0, 0), 50);
        assert_eq!(cog.expected_value(15, 15), 50);

        // Cell (1,0) should be 200
        assert_eq!(cog.expected_value(16, 0), 200);
        assert_eq!(cog.expected_value(31, 15), 200);

        // Cell (0,1) should be 200
        assert_eq!(cog.expected_value(0, 16), 200);

        // Cell (1,1) should be 50
        assert_eq!(cog.expected_value(16, 16), 50);
    }

    #[test]
    fn test_pixel_geo_roundtrip() {
        let cog = TestCogGenerator::new().world_bounds().size(1024, 1024);

        // Test center pixel
        let (geo_x, geo_y) = cog.pixel_to_geo(512, 512);
        let (px, py) = cog.geo_to_pixel(geo_x, geo_y);
        assert_eq!(px, 512);
        assert_eq!(py, 512);

        // Test corner
        let (geo_x, geo_y) = cog.pixel_to_geo(0, 0);
        let (px, py) = cog.geo_to_pixel(geo_x, geo_y);
        assert_eq!(px, 0);
        assert_eq!(py, 0);
    }

    #[test]
    fn test_generate_pixels() {
        let cog = TestCogGenerator::new().size(16, 16).pattern(TestPattern::PositionEncoded);

        let pixels = cog.generate_pixels();
        assert_eq!(pixels.len(), 256);

        // Verify first row
        for x in 0..16 {
            assert_eq!(pixels[x], cog.expected_value(x as u32, 0));
        }

        // Verify first column
        for y in 0..16 {
            assert_eq!(pixels[y * 16], cog.expected_value(0, y as u32));
        }
    }

    #[test]
    fn test_write_tiff() {
        let cog = TestCogGenerator::new()
            .size(64, 64)
            .pattern(TestPattern::PositionEncoded);

        let dir = std::env::temp_dir();
        let path = dir.join("test_generated.tif");

        cog.write_tiff(&path).expect("Failed to write TIFF");

        // Verify file exists and has reasonable size
        let metadata = std::fs::metadata(&path).expect("File should exist");
        assert!(metadata.len() > 100, "File should have content");

        // Clean up
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_gradient_patterns() {
        let cog_h = TestCogGenerator::new()
            .size(256, 256)
            .pattern(TestPattern::HorizontalGradient);

        let cog_v = TestCogGenerator::new()
            .size(256, 256)
            .pattern(TestPattern::VerticalGradient);

        // Horizontal gradient: same value in column
        assert_eq!(cog_h.expected_value(0, 0), cog_h.expected_value(0, 100));
        // Different values across row
        assert!(cog_h.expected_value(0, 0) < cog_h.expected_value(255, 0));

        // Vertical gradient: same value in row
        assert_eq!(cog_v.expected_value(0, 0), cog_v.expected_value(100, 0));
        // Different values down column
        assert!(cog_v.expected_value(0, 0) < cog_v.expected_value(0, 255));
    }
}
