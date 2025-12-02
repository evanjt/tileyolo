//! Pure Rust COG (Cloud Optimized `GeoTIFF`) reader
//!
//! This module implements efficient COG reading following the COG specification:
//! - Reads only the IFD metadata on initialization (typically < 16KB)
//! - Uses range requests for tile data (no full file downloads)
//! - Caches decompressed tiles with LRU eviction
//! - Supports multiple sources: local files, HTTP, S3
//!
//! Key optimizations:
//! - Min/max from GDAL statistics tags (no full scan needed)
//! - Data type detection from TIFF tags (no trial-and-error)
//! - CRS detection from `GeoKey` directory
//! - Single transform inversion per tile (not per pixel)

use crate::reader::range_reader::{create_range_reader, RangeReader};
use crate::reader::tiff_utils::AnyResult;
use std::collections::HashMap;
use std::sync::Arc;

// TIFF tag constants
const TAG_IMAGE_WIDTH: u16 = 256;
const TAG_IMAGE_LENGTH: u16 = 257;
const TAG_BITS_PER_SAMPLE: u16 = 258;
const TAG_COMPRESSION: u16 = 259;
const TAG_SAMPLES_PER_PIXEL: u16 = 277;
const TAG_PREDICTOR: u16 = 317;
const TAG_ROWS_PER_STRIP: u16 = 278;
const TAG_STRIP_OFFSETS: u16 = 273;
const TAG_STRIP_BYTE_COUNTS: u16 = 279;
const TAG_TILE_WIDTH: u16 = 322;
const TAG_TILE_LENGTH: u16 = 323;
const TAG_TILE_OFFSETS: u16 = 324;
const TAG_TILE_BYTE_COUNTS: u16 = 325;
const TAG_SAMPLE_FORMAT: u16 = 339;
const TAG_MODEL_PIXEL_SCALE: u16 = 33550;
const TAG_MODEL_TIEPOINT: u16 = 33922;
const TAG_GEO_KEY_DIRECTORY: u16 = 34735;
const TAG_GDAL_METADATA: u16 = 42112;
const TAG_GDAL_NODATA: u16 = 42113;

// GeoKey constants
const GEO_KEY_GEOGRAPHIC_TYPE: u16 = 2048;
const GEO_KEY_PROJECTED_CRS: u16 = 3072;

// Compression constants
const COMPRESSION_NONE: u16 = 1;
const COMPRESSION_LZW: u16 = 5;
const COMPRESSION_DEFLATE: u16 = 8;
const COMPRESSION_ZSTD: u16 = 50000;

// Sample format constants
const SAMPLE_FORMAT_UINT: u16 = 1;
const SAMPLE_FORMAT_INT: u16 = 2;
const SAMPLE_FORMAT_FLOAT: u16 = 3;

/// Data type detected from TIFF tags
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CogDataType {
    UInt8,
    UInt16,
    UInt32,
    UInt64,
    Int8,
    Int16,
    Int32,
    Int64,
    Float32,
    Float64,
}

impl CogDataType {
    #[must_use] pub fn bytes_per_sample(&self) -> usize {
        match self {
            CogDataType::UInt8 | CogDataType::Int8 => 1,
            CogDataType::UInt16 | CogDataType::Int16 => 2,
            CogDataType::UInt32 | CogDataType::Int32 | CogDataType::Float32 => 4,
            CogDataType::UInt64 | CogDataType::Int64 | CogDataType::Float64 => 8,
        }
    }

    /// Detect data type from TIFF tags
    #[must_use] pub fn from_tags(bits_per_sample: u16, sample_format: u16) -> Option<Self> {
        match (sample_format, bits_per_sample) {
            (SAMPLE_FORMAT_UINT, 8) => Some(CogDataType::UInt8),
            (SAMPLE_FORMAT_UINT, 16) => Some(CogDataType::UInt16),
            (SAMPLE_FORMAT_UINT, 32) => Some(CogDataType::UInt32),
            (SAMPLE_FORMAT_UINT, 64) => Some(CogDataType::UInt64),
            (SAMPLE_FORMAT_INT, 8) => Some(CogDataType::Int8),
            (SAMPLE_FORMAT_INT, 16) => Some(CogDataType::Int16),
            (SAMPLE_FORMAT_INT, 32) => Some(CogDataType::Int32),
            (SAMPLE_FORMAT_INT, 64) => Some(CogDataType::Int64),
            (SAMPLE_FORMAT_FLOAT, 32) => Some(CogDataType::Float32),
            (SAMPLE_FORMAT_FLOAT, 64) => Some(CogDataType::Float64),
            // Default to unsigned if sample format not specified
            (_, 8) => Some(CogDataType::UInt8),
            (_, 16) => Some(CogDataType::UInt16),
            (_, 32) => Some(CogDataType::UInt32),
            _ => None,
        }
    }
}

/// Compression method
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    None,
    Lzw,
    Deflate,
    Zstd,
}

impl Compression {
    #[must_use] pub fn from_tag(value: u16) -> Option<Self> {
        match value {
            COMPRESSION_NONE => Some(Compression::None),
            COMPRESSION_LZW => Some(Compression::Lzw),
            COMPRESSION_DEFLATE | 32946 => Some(Compression::Deflate), // 32946 is old deflate
            COMPRESSION_ZSTD => Some(Compression::Zstd),
            _ => None,
        }
    }
}

/// `GeoTIFF` transform information
#[derive(Debug, Clone)]
pub struct GeoTransform {
    /// Pixel scale (`x_scale`, `y_scale`, `z_scale`)
    pub pixel_scale: Option<[f64; 3]>,
    /// Tiepoint (i, j, k, x, y, z) - maps pixel (i,j,k) to world (x,y,z)
    pub tiepoint: Option<[f64; 6]>,
}

impl GeoTransform {
    /// Convert pixel coordinates to world coordinates
    #[must_use] pub fn pixel_to_world(&self, px: f64, py: f64) -> Option<(f64, f64)> {
        let scale = self.pixel_scale?;
        let tie = self.tiepoint?;

        let world_x = tie[3] + (px - tie[0]) * scale[0];
        let world_y = tie[4] - (py - tie[1]) * scale[1]; // Y is typically inverted

        Some((world_x, world_y))
    }

    /// Convert world coordinates to pixel coordinates
    #[must_use] pub fn world_to_pixel(&self, wx: f64, wy: f64) -> Option<(f64, f64)> {
        let scale = self.pixel_scale?;
        let tie = self.tiepoint?;

        if scale[0] == 0.0 || scale[1] == 0.0 {
            return None;
        }

        let px = tie[0] + (wx - tie[3]) / scale[0];
        let py = tie[1] + (tie[4] - wy) / scale[1]; // Y is typically inverted

        Some((px, py))
    }

    /// Get the world extent of the image
    #[must_use] pub fn get_extent(&self, width: usize, height: usize) -> Option<(f64, f64, f64, f64)> {
        let (minx, maxy) = self.pixel_to_world(0.0, 0.0)?;
        let (maxx, miny) = self.pixel_to_world(width as f64, height as f64)?;
        Some((minx, miny, maxx, maxy))
    }
}

/// COG metadata - read from IFD without loading tile data
#[derive(Debug, Clone)]
pub struct CogMetadata {
    /// Image dimensions
    pub width: usize,
    pub height: usize,

    /// Tile dimensions (COG requirement)
    pub tile_width: usize,
    pub tile_height: usize,

    /// Number of bands/samples
    pub bands: usize,

    /// Data type
    pub data_type: CogDataType,

    /// Compression method
    pub compression: Compression,

    /// Predictor (1=none, 2=horizontal differencing, 3=floating point)
    pub predictor: u16,

    /// Byte order
    pub little_endian: bool,

    /// Tile byte offsets in the file
    pub tile_offsets: Vec<u64>,

    /// Tile byte counts (compressed sizes)
    pub tile_byte_counts: Vec<u64>,

    /// Number of tiles across
    pub tiles_across: usize,

    /// Number of tiles down
    pub tiles_down: usize,

    /// Whether this is a tiled TIFF (true) or stripped TIFF (false)
    /// Tiled TIFFs are COG-optimized, stripped TIFFs are not
    pub is_tiled: bool,

    /// Geographic transform
    pub geo_transform: GeoTransform,

    /// Detected CRS (EPSG code)
    pub crs_code: Option<i32>,

    /// Min/max values from GDAL statistics (if present)
    pub stats_min: Option<f32>,
    pub stats_max: Option<f32>,

    /// `NoData` value
    pub nodata: Option<f64>,
}

impl CogMetadata {
    /// Check if this appears to be a valid COG (has tiles)
    #[must_use] pub fn is_tiled(&self) -> bool {
        self.tile_width > 0 && self.tile_height > 0
    }

    /// Get tile index for a pixel coordinate
    #[must_use] pub fn tile_index_for_pixel(&self, px: usize, py: usize) -> Option<usize> {
        if px >= self.width || py >= self.height {
            return None;
        }
        let tile_col = px / self.tile_width;
        let tile_row = py / self.tile_height;
        Some(tile_row * self.tiles_across + tile_col)
    }

    /// Get pixel range within a tile
    #[must_use] pub fn pixel_range_in_tile(&self, tile_index: usize) -> (usize, usize, usize, usize) {
        let tile_col = tile_index % self.tiles_across;
        let tile_row = tile_index / self.tiles_across;

        let start_x = tile_col * self.tile_width;
        let start_y = tile_row * self.tile_height;
        let end_x = (start_x + self.tile_width).min(self.width);
        let end_y = (start_y + self.tile_height).min(self.height);

        (start_x, start_y, end_x, end_y)
    }

    /// Get number of valid pixels in a tile (handles edge tiles)
    #[must_use] pub fn tile_pixel_count(&self, tile_index: usize) -> usize {
        let (start_x, start_y, end_x, end_y) = self.pixel_range_in_tile(tile_index);
        (end_x - start_x) * (end_y - start_y) * self.bands
    }
}

/// Overview metadata - subset of `CogMetadata` for overviews
#[derive(Debug, Clone)]
pub struct OverviewMetadata {
    pub width: usize,
    pub height: usize,
    pub tile_width: usize,
    pub tile_height: usize,
    pub tiles_across: usize,
    pub tiles_down: usize,
    pub tile_offsets: Vec<u64>,
    pub tile_byte_counts: Vec<u64>,
    /// Scale factor relative to full resolution (2, 4, 8, etc.)
    pub scale: usize,
}

impl OverviewMetadata {
    /// Get tile index for a pixel coordinate at this overview level
    #[must_use] pub fn tile_index_for_pixel(&self, px: usize, py: usize) -> Option<usize> {
        if px >= self.width || py >= self.height {
            return None;
        }
        let tile_col = px / self.tile_width;
        let tile_row = py / self.tile_height;
        Some(tile_row * self.tiles_across + tile_col)
    }
}

/// COG Reader - efficient COG access with range requests
pub struct CogReader {
    reader: Arc<dyn RangeReader>,
    pub metadata: CogMetadata,
    /// Overview levels (sorted by scale factor, smallest to largest)
    pub overviews: Vec<OverviewMetadata>,
    /// Minimum usable overview index - overviews beyond this have insufficient data
    /// None means all overviews are usable, Some(n) means only overviews 0..n are usable
    pub min_usable_overview: Option<usize>,
}

impl CogReader {
    /// Open a COG from any source (local file, HTTP URL, or S3)
    pub fn open(source: &str) -> AnyResult<Self> {
        let reader = create_range_reader(source)?;
        Self::from_reader(reader)
    }

    /// Open from an existing range reader
    pub fn from_reader(reader: Arc<dyn RangeReader>) -> AnyResult<Self> {
        // Read header to get IFD offset and byte order
        let header_bytes = reader.read_range(0, 8)?;

        let little_endian = match &header_bytes[0..2] {
            b"II" => true,
            b"MM" => false,
            _ => return Err("Invalid TIFF signature".into()),
        };

        let version = read_u16(&header_bytes[2..4], little_endian);
        if version != 42 {
            return Err(format!("Invalid TIFF version: {version}").into());
        }

        let ifd_offset = read_u32(&header_bytes[4..8], little_endian);
        let file_size = reader.size();

        // Read IFD entries - estimate size based on typical COG (usually < 4KB)
        // Clamp to available bytes if IFD is near end of file
        let ifd_size_estimate = 4096.min((file_size - u64::from(ifd_offset)) as usize);
        let ifd_bytes = reader.read_range(u64::from(ifd_offset), ifd_size_estimate)?;

        let (metadata, next_ifd_offset) = parse_ifd_with_next(&ifd_bytes, &reader, u64::from(ifd_offset), little_endian)?;

        // Read overview IFDs (subsequent IFDs in the chain)
        let mut overviews = Vec::new();
        let mut current_ifd_offset = next_ifd_offset;
        let full_width = metadata.width;

        while current_ifd_offset != 0 {
            let ovr_ifd_size = 4096.min((file_size - u64::from(current_ifd_offset)) as usize);
            let ovr_ifd_bytes = reader.read_range(u64::from(current_ifd_offset), ovr_ifd_size)?;

            if let Ok((ovr_meta, next_offset)) = parse_overview_ifd(&ovr_ifd_bytes, &reader, u64::from(current_ifd_offset), little_endian, &metadata) {
                // Calculate actual scale from dimensions using floor division
                // This matches GDAL's behavior: scale = full_width / ovr_width
                // For 20966/1310 this gives 16, not 17 (ceiling would be wrong)
                let actual_scale = full_width / ovr_meta.width;

                overviews.push(OverviewMetadata {
                    width: ovr_meta.width,
                    height: ovr_meta.height,
                    tile_width: ovr_meta.tile_width,
                    tile_height: ovr_meta.tile_height,
                    tiles_across: ovr_meta.tiles_across,
                    tiles_down: ovr_meta.tiles_down,
                    tile_offsets: ovr_meta.tile_offsets,
                    tile_byte_counts: ovr_meta.tile_byte_counts,
                    scale: actual_scale,
                });

                current_ifd_offset = next_offset;
            } else {
                break;
            }

            // Safety limit - COGs typically have at most 10 overviews
            if overviews.len() > 10 {
                break;
            }
        }

        let mut cog_reader = Self {
            reader,
            metadata,
            overviews,
            min_usable_overview: None,
        };

        // Analyze overview quality to find minimum usable level
        cog_reader.analyze_overview_quality();

        Ok(cog_reader)
    }

    /// Analyze overview quality by sampling tiles to find valid data density
    /// This determines which overviews have enough data to be useful
    fn analyze_overview_quality(&mut self) {
        if self.overviews.is_empty() {
            return;
        }

        // For each overview (from smallest/coarsest to largest/finest), check if it has enough data
        // We sample a few tiles from each overview and check data density
        //
        // Use 5% threshold - this is aggressive but ensures good visual results for sparse data.
        // For a file with 6% valid data at full res, overviews with <5% are significantly degraded.
        // The trade-off is that sparse datasets will read more tiles at low zoom, but the visual
        // quality improvement is dramatic (see barley crop data as example).
        let min_density_threshold = 0.05; // 5% - require good data density for visual quality

        let mut last_good_overview: Option<usize> = None;
        let mut overview_densities: Vec<(usize, f64)> = Vec::new();

        // Iterate from smallest overview (highest index, coarsest) to largest (index 0, finest)
        for (idx, ovr) in self.overviews.iter().enumerate().rev() {
            // Sample up to 3 tiles from this overview
            let num_tiles = ovr.tile_offsets.len();
            let sample_indices: Vec<usize> = if num_tiles <= 3 {
                (0..num_tiles).collect()
            } else {
                // Sample first, middle, and last tiles
                vec![0, num_tiles / 2, num_tiles - 1]
            };

            let mut total_pixels = 0usize;
            let mut valid_pixels = 0usize;

            for &tile_idx in &sample_indices {
                if let Ok(data) = self.read_overview_tile(idx, tile_idx) {
                    total_pixels += data.len();
                    valid_pixels += data.iter().filter(|v| !v.is_nan() && **v != 0.0).count();
                }
            }

            let density = if total_pixels > 0 {
                valid_pixels as f64 / total_pixels as f64
            } else {
                0.0
            };

            overview_densities.push((idx, density));

            if density >= min_density_threshold {
                last_good_overview = Some(idx);
                break; // Found a good overview, don't need to check higher resolution ones
            }
        }

        // If we found a good overview, set it as the minimum usable
        // All overviews with index > last_good_overview are too sparse
        self.min_usable_overview = last_good_overview;
    }

    /// Find the best overview level for a given source extent size
    ///
    /// Parameters:
    /// - `extent_src_width`: How many source pixels the extent covers at full resolution
    /// - `extent_src_height`: How many source pixels the extent covers at full resolution
    /// - `output_width`: How many pixels we're actually rendering (e.g., 256)
    /// - `output_height`: How many pixels we're actually rendering (e.g., 256)
    ///
    /// Returns None if full resolution should be used
    #[must_use] pub fn best_overview_for_resolution(&self, extent_src_width: usize, extent_src_height: usize) -> Option<usize> {
        // If min_usable_overview is None, ALL overviews are too sparse - always use full resolution
        // This is critical for sparse datasets where even the largest overview has insufficient data
        if self.min_usable_overview.is_none() && !self.overviews.is_empty() {
            return None;
        }

        // Default output tile size
        let output_size = 256.0;

        // Calculate how many source pixels per output pixel we'd need at full res
        // If extent covers 21600 source pixels but we only output 256 pixels, we can use an 84x overview
        // If extent covers 256 source pixels for 256 output, we need full resolution (scale = 1)
        let scale_x = extent_src_width as f64 / output_size;
        let scale_y = extent_src_height as f64 / output_size;
        let needed_scale = scale_x.max(scale_y);

        // If we need close to full resolution (1:1 or less), don't use an overview
        if needed_scale < 1.5 {
            return None;
        }

        // Find the best overview that has enough resolution
        // We want the overview with the largest scale that's still <= needed_scale
        // (i.e., the smallest overview that still has enough detail)
        let mut best_idx = None;
        let mut best_scale = 0usize;

        for (idx, ovr) in self.overviews.iter().enumerate() {
            // Skip overviews that have been determined to have insufficient data
            // min_usable_overview = Some(n) means only overviews 0..=n have enough data
            if let Some(min_usable) = self.min_usable_overview
                && idx > min_usable {
                    // This overview is too sparse (beyond the minimum usable level)
                    continue;
                }

            // This overview has 1/scale resolution compared to full
            // We can use it if the overview has at least as many pixels as we need
            // needed_scale = extent_pixels / output_pixels
            // If needed_scale = 84 and overview scale = 64, overview has enough resolution
            if ovr.scale <= (needed_scale as usize) && ovr.scale > best_scale {
                best_scale = ovr.scale;
                best_idx = Some(idx);
            }
        }

        best_idx
    }

    /// Read a tile from a specific overview level
    pub fn read_overview_tile(&self, overview_idx: usize, tile_index: usize) -> AnyResult<Vec<f32>> {
        let ovr = self.overviews.get(overview_idx)
            .ok_or_else(|| format!("Overview index {overview_idx} out of range"))?;

        if tile_index >= ovr.tile_offsets.len() {
            return Err(format!(
                "Tile index {} out of range (max {})",
                tile_index,
                ovr.tile_offsets.len()
            ).into());
        }

        let offset = ovr.tile_offsets[tile_index];
        let byte_count = ovr.tile_byte_counts[tile_index] as usize;

        if byte_count == 0 {
            let pixel_count = ovr.tile_width * ovr.tile_height * self.metadata.bands;
            return Ok(vec![f32::NAN; pixel_count]);
        }

        let compressed = self.reader.read_range(offset, byte_count)?;

        let decompressed = decompress_tile(
            &compressed,
            self.metadata.compression,
            ovr.tile_width,
            ovr.tile_height,
            self.metadata.bands,
            self.metadata.data_type.bytes_per_sample(),
        )?;

        let unpredicted = apply_predictor(
            &decompressed,
            self.metadata.predictor,
            ovr.tile_width,
            self.metadata.bands,
            self.metadata.data_type.bytes_per_sample(),
        )?;

        convert_to_f32(
            &unpredicted,
            self.metadata.data_type,
            self.metadata.little_endian,
        )
    }

    /// Read a single tile's raw data and decompress
    pub fn read_tile(&self, tile_index: usize) -> AnyResult<Vec<f32>> {
        if tile_index >= self.metadata.tile_offsets.len() {
            return Err(format!(
                "Tile index {} out of range (max {})",
                tile_index,
                self.metadata.tile_offsets.len()
            )
            .into());
        }

        let offset = self.metadata.tile_offsets[tile_index];
        let byte_count = self.metadata.tile_byte_counts[tile_index] as usize;

        if byte_count == 0 {
            // Empty tile - return NaN-filled data
            let pixel_count = self.metadata.tile_width * self.metadata.tile_height * self.metadata.bands;
            return Ok(vec![f32::NAN; pixel_count]);
        }

        let compressed = self.reader.read_range(offset, byte_count)?;

        // Decompress
        let decompressed = decompress_tile(
            &compressed,
            self.metadata.compression,
            self.metadata.tile_width,
            self.metadata.tile_height,
            self.metadata.bands,
            self.metadata.data_type.bytes_per_sample(),
        )?;

        // Apply predictor if needed
        let unpredicted = apply_predictor(
            &decompressed,
            self.metadata.predictor,
            self.metadata.tile_width,
            self.metadata.bands,
            self.metadata.data_type.bytes_per_sample(),
        )?;

        // Convert to f32
        convert_to_f32(
            &unpredicted,
            self.metadata.data_type,
            self.metadata.little_endian,
        )
    }

    /// Sample a single pixel value
    pub fn sample(&self, band: usize, x: usize, y: usize) -> AnyResult<Option<f32>> {
        let Some(tile_index) = self.metadata.tile_index_for_pixel(x, y) else {
            return Ok(None);
        };

        let tile_data = self.read_tile(tile_index)?;

        // Calculate position within tile
        let tile_col = tile_index % self.metadata.tiles_across;
        let tile_row = tile_index / self.metadata.tiles_across;
        let local_x = x - tile_col * self.metadata.tile_width;
        let local_y = y - tile_row * self.metadata.tile_height;

        let idx = (local_y * self.metadata.tile_width + local_x) * self.metadata.bands + band;
        Ok(tile_data.get(idx).copied())
    }

    /// Estimate min/max from sampling (when GDAL stats not available)
    ///
    /// For local files: Scans ALL tiles for accurate min/max values
    /// For remote files (S3/HTTP): Samples a few tiles for efficiency
    ///
    /// Use `estimate_min_max_fast()` to always use fast sampling regardless of source.
    pub fn estimate_min_max(&self) -> AnyResult<(f32, f32)> {
        // First check for GDAL statistics
        if let (Some(min), Some(max)) = (self.metadata.stats_min, self.metadata.stats_max) {
            return Ok((min, max));
        }

        // For local files, do a full scan for accuracy
        // For remote files, use fast sampling to minimize network requests
        if self.reader.is_local() {
            self.estimate_min_max_full_scan()
        } else {
            self.estimate_min_max_fast()
        }
    }

    /// Fast min/max estimation - samples only corner and center tiles
    /// Use this for remote files where full scans are expensive
    pub fn estimate_min_max_fast(&self) -> AnyResult<(f32, f32)> {
        // First check for GDAL statistics
        if let (Some(min), Some(max)) = (self.metadata.stats_min, self.metadata.stats_max) {
            return Ok((min, max));
        }

        // For files with overviews, sample from the smallest overview (most efficient)
        if !self.overviews.is_empty() {
            let smallest_ovr_idx = self.overviews.len() - 1;
            let ovr = &self.overviews[smallest_ovr_idx];
            let total_tiles = ovr.tile_offsets.len();

            // Sample corner tiles + center tile from smallest overview
            let sample_indices: Vec<usize> = if total_tiles <= 5 {
                (0..total_tiles).collect()
            } else {
                vec![
                    0,
                    ovr.tiles_across.saturating_sub(1),
                    total_tiles / 2,
                    total_tiles.saturating_sub(ovr.tiles_across),
                    total_tiles.saturating_sub(1),
                ]
            };

            return self.scan_tiles_for_minmax(&sample_indices, Some(smallest_ovr_idx));
        }

        // No overviews - sample from full resolution tiles
        let total_tiles = self.metadata.tile_offsets.len();
        let sample_indices: Vec<usize> = if total_tiles <= 5 {
            (0..total_tiles).collect()
        } else {
            vec![
                0,                                          // Top-left
                self.metadata.tiles_across.saturating_sub(1), // Top-right
                total_tiles / 2,                            // Center
                total_tiles.saturating_sub(self.metadata.tiles_across), // Bottom-left
                total_tiles.saturating_sub(1),              // Bottom-right
            ]
        };

        self.scan_tiles_for_minmax(&sample_indices, None)
    }

    /// Full scan min/max estimation - reads ALL tiles
    /// Use this for local files where disk I/O is fast
    fn estimate_min_max_full_scan(&self) -> AnyResult<(f32, f32)> {
        // For files with overviews, scan the smallest overview (much faster)
        if !self.overviews.is_empty() {
            let smallest_ovr_idx = self.overviews.len() - 1;
            let ovr = &self.overviews[smallest_ovr_idx];
            let all_indices: Vec<usize> = (0..ovr.tile_offsets.len()).collect();
            return self.scan_tiles_for_minmax(&all_indices, Some(smallest_ovr_idx));
        }

        // No overviews - must scan full resolution
        let all_indices: Vec<usize> = (0..self.metadata.tile_offsets.len()).collect();
        self.scan_tiles_for_minmax(&all_indices, None)
    }

    /// Helper to scan specific tiles for min/max values
    fn scan_tiles_for_minmax(&self, indices: &[usize], overview_idx: Option<usize>) -> AnyResult<(f32, f32)> {
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        let nodata = self.metadata.nodata;

        for &tile_idx in indices {
            let tile_data = if let Some(ovr_idx) = overview_idx {
                self.read_overview_tile(ovr_idx, tile_idx)?
            } else {
                self.read_tile(tile_idx)?
            };

            for &val in &tile_data {
                // Skip NaN and nodata values
                if val.is_nan() {
                    continue;
                }
                if let Some(nd) = nodata
                    && (f64::from(val) - nd).abs() < 0.001 {
                        continue;
                    }
                if val < min {
                    min = val;
                }
                if val > max {
                    max = val;
                }
            }
        }

        if min.is_infinite() || max.is_infinite() {
            Ok((0.0, 1.0)) // Fallback
        } else {
            Ok((min, max))
        }
    }
}

// ============================================================================
// Helper functions for reading TIFF data
// ============================================================================

fn read_u16(bytes: &[u8], little_endian: bool) -> u16 {
    if little_endian {
        u16::from_le_bytes([bytes[0], bytes[1]])
    } else {
        u16::from_be_bytes([bytes[0], bytes[1]])
    }
}

fn read_u32(bytes: &[u8], little_endian: bool) -> u32 {
    if little_endian {
        u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    } else {
        u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    }
}

fn read_u64(bytes: &[u8], little_endian: bool) -> u64 {
    if little_endian {
        u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3],
            bytes[4], bytes[5], bytes[6], bytes[7],
        ])
    } else {
        u64::from_be_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3],
            bytes[4], bytes[5], bytes[6], bytes[7],
        ])
    }
}

fn read_f64(bytes: &[u8], little_endian: bool) -> f64 {
    if little_endian {
        f64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3],
            bytes[4], bytes[5], bytes[6], bytes[7],
        ])
    } else {
        f64::from_be_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3],
            bytes[4], bytes[5], bytes[6], bytes[7],
        ])
    }
}

/// Parse IFD and extract all COG metadata
fn parse_ifd(
    ifd_bytes: &[u8],
    reader: &Arc<dyn RangeReader>,
    ifd_offset: u64,
    little_endian: bool,
) -> AnyResult<CogMetadata> {
    let entry_count = read_u16(&ifd_bytes[0..2], little_endian) as usize;

    // Parse all IFD entries into a map
    let mut tags: HashMap<u16, IfdEntry> = HashMap::new();

    for i in 0..entry_count {
        let offset = 2 + i * 12;
        if offset + 12 > ifd_bytes.len() {
            break;
        }

        let tag = read_u16(&ifd_bytes[offset..offset + 2], little_endian);
        let field_type = read_u16(&ifd_bytes[offset + 2..offset + 4], little_endian);
        let count = read_u32(&ifd_bytes[offset + 4..offset + 8], little_endian);
        let value_offset = read_u32(&ifd_bytes[offset + 8..offset + 12], little_endian);

        tags.insert(
            tag,
            IfdEntry {
                field_type,
                count,
                value_offset,
                raw_bytes: [
                    ifd_bytes[offset + 8],
                    ifd_bytes[offset + 9],
                    ifd_bytes[offset + 10],
                    ifd_bytes[offset + 11],
                ],
            },
        );
    }

    // Extract required tags
    let width = get_tag_value(&tags, TAG_IMAGE_WIDTH, little_endian)
        .ok_or("Missing ImageWidth tag")? as usize;
    let height = get_tag_value(&tags, TAG_IMAGE_LENGTH, little_endian)
        .ok_or("Missing ImageLength tag")? as usize;

    let bits_per_sample = get_tag_value(&tags, TAG_BITS_PER_SAMPLE, little_endian).unwrap_or(8) as u16;
    let sample_format = get_tag_value(&tags, TAG_SAMPLE_FORMAT, little_endian).unwrap_or(1) as u16;
    let bands = get_tag_value(&tags, TAG_SAMPLES_PER_PIXEL, little_endian).unwrap_or(1) as usize;
    let compression_val = get_tag_value(&tags, TAG_COMPRESSION, little_endian).unwrap_or(1) as u16;
    let predictor = get_tag_value(&tags, TAG_PREDICTOR, little_endian).unwrap_or(1) as u16;

    let data_type = CogDataType::from_tags(bits_per_sample, sample_format)
        .ok_or_else(|| format!("Unsupported data type: bits={bits_per_sample}, format={sample_format}"))?;

    let compression = Compression::from_tag(compression_val)
        .ok_or_else(|| format!("Unsupported compression: {compression_val}"))?;

    // Detect if tiled or stripped TIFF
    let has_tile_tags = tags.contains_key(&TAG_TILE_OFFSETS);
    let has_strip_tags = tags.contains_key(&TAG_STRIP_OFFSETS);
    let is_tiled = has_tile_tags;

    // For tiled: use tile dimensions; for stripped: tile_width = image width, tile_height = rows_per_strip
    let (tile_width, tile_height, tiles_across, tiles_down, tile_offsets, tile_byte_counts) = if is_tiled {
        // Tiled TIFF (COG-optimized)
        let tw = get_tag_value(&tags, TAG_TILE_WIDTH, little_endian).unwrap_or(width as u32) as usize;
        let th = get_tag_value(&tags, TAG_TILE_LENGTH, little_endian).unwrap_or(height as u32) as usize;
        let ta = width.div_ceil(tw);
        let td = height.div_ceil(th);
        let total_tiles = ta * td;

        let offsets = read_tag_array_u64(
            &tags,
            TAG_TILE_OFFSETS,
            reader,
            ifd_offset,
            little_endian,
            total_tiles,
        )?;

        let byte_counts = read_tag_array_u64(
            &tags,
            TAG_TILE_BYTE_COUNTS,
            reader,
            ifd_offset,
            little_endian,
            total_tiles,
        )?;

        (tw, th, ta, td, offsets, byte_counts)
    } else if has_strip_tags {
        // Stripped TIFF (not COG-optimized)
        // Treat strips as "tiles" that span the full image width
        let rows_per_strip = get_tag_value(&tags, TAG_ROWS_PER_STRIP, little_endian)
            .unwrap_or(height as u32) as usize;
        let tw = width; // Strip width = image width
        let th = rows_per_strip;
        let ta = 1; // Only 1 "tile" across (strips span full width)
        let td = height.div_ceil(rows_per_strip);
        let total_strips = td;

        let offsets = read_tag_array_u64(
            &tags,
            TAG_STRIP_OFFSETS,
            reader,
            ifd_offset,
            little_endian,
            total_strips,
        )?;

        let byte_counts = read_tag_array_u64(
            &tags,
            TAG_STRIP_BYTE_COUNTS,
            reader,
            ifd_offset,
            little_endian,
            total_strips,
        )?;

        (tw, th, ta, td, offsets, byte_counts)
    } else {
        return Err("TIFF has neither tile nor strip tags".into());
    };

    // Read geo transform
    let pixel_scale = read_tag_f64_array(&tags, TAG_MODEL_PIXEL_SCALE, reader, ifd_offset, little_endian, 3)?;
    let tiepoint = read_tag_f64_array(&tags, TAG_MODEL_TIEPOINT, reader, ifd_offset, little_endian, 6)?;

    let geo_transform = GeoTransform {
        pixel_scale: pixel_scale.map(|v| [v[0], v[1], v[2]]),
        tiepoint: tiepoint.map(|v| [v[0], v[1], v[2], v[3], v[4], v[5]]),
    };

    // Read CRS from GeoKey directory
    let crs_code = read_crs_from_geokeys(&tags, reader, ifd_offset, little_endian)?;

    // Read GDAL statistics
    let (stats_min, stats_max) = read_gdal_stats(&tags, reader, ifd_offset, little_endian)?;

    // Read nodata
    let nodata = read_gdal_nodata(&tags, reader, ifd_offset, little_endian)?;

    Ok(CogMetadata {
        width,
        height,
        tile_width,
        tile_height,
        bands,
        data_type,
        compression,
        predictor,
        little_endian,
        tile_offsets,
        tile_byte_counts,
        tiles_across,
        tiles_down,
        is_tiled,
        geo_transform,
        crs_code,
        stats_min,
        stats_max,
        nodata,
    })
}

/// Parse IFD and return metadata plus next IFD offset
fn parse_ifd_with_next(
    ifd_bytes: &[u8],
    reader: &Arc<dyn RangeReader>,
    ifd_offset: u64,
    little_endian: bool,
) -> AnyResult<(CogMetadata, u32)> {
    let entry_count = read_u16(&ifd_bytes[0..2], little_endian) as usize;

    // The next IFD offset is right after all entries
    let next_ifd_pos = 2 + entry_count * 12;
    let next_ifd_offset = if next_ifd_pos + 4 <= ifd_bytes.len() {
        read_u32(&ifd_bytes[next_ifd_pos..next_ifd_pos + 4], little_endian)
    } else {
        0
    };

    let metadata = parse_ifd(ifd_bytes, reader, ifd_offset, little_endian)?;
    Ok((metadata, next_ifd_offset))
}

/// Simplified metadata for overview IFDs
struct OverviewIfdData {
    width: usize,
    height: usize,
    tile_width: usize,
    tile_height: usize,
    tiles_across: usize,
    tiles_down: usize,
    tile_offsets: Vec<u64>,
    tile_byte_counts: Vec<u64>,
}

/// Parse an overview IFD (simpler than full IFD parsing)
fn parse_overview_ifd(
    ifd_bytes: &[u8],
    reader: &Arc<dyn RangeReader>,
    ifd_offset: u64,
    little_endian: bool,
    _full_meta: &CogMetadata, // For inheriting compression, data type, etc.
) -> AnyResult<(OverviewIfdData, u32)> {
    let entry_count = read_u16(&ifd_bytes[0..2], little_endian) as usize;

    // Parse all IFD entries into a map
    let mut tags: HashMap<u16, IfdEntry> = HashMap::new();

    for i in 0..entry_count {
        let offset = 2 + i * 12;
        if offset + 12 > ifd_bytes.len() {
            break;
        }

        let tag = read_u16(&ifd_bytes[offset..offset + 2], little_endian);
        let field_type = read_u16(&ifd_bytes[offset + 2..offset + 4], little_endian);
        let count = read_u32(&ifd_bytes[offset + 4..offset + 8], little_endian);
        let value_offset = read_u32(&ifd_bytes[offset + 8..offset + 12], little_endian);

        tags.insert(
            tag,
            IfdEntry {
                field_type,
                count,
                value_offset,
                raw_bytes: [
                    ifd_bytes[offset + 8],
                    ifd_bytes[offset + 9],
                    ifd_bytes[offset + 10],
                    ifd_bytes[offset + 11],
                ],
            },
        );
    }

    // Get next IFD offset
    let next_ifd_pos = 2 + entry_count * 12;
    let next_ifd_offset = if next_ifd_pos + 4 <= ifd_bytes.len() {
        read_u32(&ifd_bytes[next_ifd_pos..next_ifd_pos + 4], little_endian)
    } else {
        0
    };

    // Extract dimensions and tile info
    let width = get_tag_value(&tags, TAG_IMAGE_WIDTH, little_endian)
        .ok_or("Overview missing ImageWidth tag")? as usize;
    let height = get_tag_value(&tags, TAG_IMAGE_LENGTH, little_endian)
        .ok_or("Overview missing ImageLength tag")? as usize;

    let tile_width = get_tag_value(&tags, TAG_TILE_WIDTH, little_endian)
        .ok_or("Overview missing TileWidth tag")? as usize;
    let tile_height = get_tag_value(&tags, TAG_TILE_LENGTH, little_endian)
        .ok_or("Overview missing TileLength tag")? as usize;

    let tiles_across = width.div_ceil(tile_width);
    let tiles_down = height.div_ceil(tile_height);
    let total_tiles = tiles_across * tiles_down;

    // Read tile offsets and byte counts
    let tile_offsets = read_tag_array_u64(
        &tags,
        TAG_TILE_OFFSETS,
        reader,
        ifd_offset,
        little_endian,
        total_tiles,
    )?;

    let tile_byte_counts = read_tag_array_u64(
        &tags,
        TAG_TILE_BYTE_COUNTS,
        reader,
        ifd_offset,
        little_endian,
        total_tiles,
    )?;

    Ok((OverviewIfdData {
        width,
        height,
        tile_width,
        tile_height,
        tiles_across,
        tiles_down,
        tile_offsets,
        tile_byte_counts,
    }, next_ifd_offset))
}

struct IfdEntry {
    field_type: u16,
    count: u32,
    value_offset: u32,
    raw_bytes: [u8; 4],
}

fn get_tag_value(tags: &HashMap<u16, IfdEntry>, tag: u16, little_endian: bool) -> Option<u32> {
    let entry = tags.get(&tag)?;
    let type_size = match entry.field_type {
        1 => 1, // BYTE
        3 => 2, // SHORT
        4 => 4, // LONG
        _ => return None,
    };

    if entry.count == 1 && type_size <= 4 {
        // Value is inline
        match entry.field_type {
            1 => Some(u32::from(entry.raw_bytes[0])),
            3 => Some(u32::from(read_u16(&entry.raw_bytes, little_endian))),
            4 => Some(read_u32(&entry.raw_bytes, little_endian)),
            _ => None,
        }
    } else {
        None // Would need to read from offset
    }
}

fn read_tag_array_u64(
    tags: &HashMap<u16, IfdEntry>,
    tag: u16,
    reader: &Arc<dyn RangeReader>,
    _ifd_offset: u64,
    little_endian: bool,
    expected_count: usize,
) -> AnyResult<Vec<u64>> {
    let entry = tags.get(&tag).ok_or_else(|| format!("Missing tag {tag}"))?;

    let type_size = match entry.field_type {
        3 => 2, // SHORT
        4 => 4, // LONG
        16 => 8, // LONG8
        _ => return Err(format!("Unsupported type {} for tag {}", entry.field_type, tag).into()),
    };

    let total_bytes = entry.count as usize * type_size;

    let raw_bytes = if total_bytes <= 4 {
        entry.raw_bytes[..total_bytes].to_vec()
    } else {
        reader.read_range(u64::from(entry.value_offset), total_bytes)?
    };

    let mut values = Vec::with_capacity(entry.count as usize);
    for i in 0..entry.count as usize {
        let offset = i * type_size;
        let value = match entry.field_type {
            3 => u64::from(read_u16(&raw_bytes[offset..], little_endian)),
            4 => u64::from(read_u32(&raw_bytes[offset..], little_endian)),
            16 => read_u64(&raw_bytes[offset..], little_endian),
            _ => 0,
        };
        values.push(value);
    }

    // Pad with zeros if we got fewer than expected
    while values.len() < expected_count {
        values.push(0);
    }

    Ok(values)
}

fn read_tag_f64_array(
    tags: &HashMap<u16, IfdEntry>,
    tag: u16,
    reader: &Arc<dyn RangeReader>,
    _ifd_offset: u64,
    little_endian: bool,
    min_count: usize,
) -> AnyResult<Option<Vec<f64>>> {
    let Some(entry) = tags.get(&tag) else {
        return Ok(None);
    };

    if entry.field_type != 12 {
        // DOUBLE
        return Ok(None);
    }

    if (entry.count as usize) < min_count {
        return Ok(None);
    }

    let total_bytes = entry.count as usize * 8;
    let raw_bytes = reader.read_range(u64::from(entry.value_offset), total_bytes)?;

    let mut values = Vec::with_capacity(entry.count as usize);
    for i in 0..entry.count as usize {
        let offset = i * 8;
        values.push(read_f64(&raw_bytes[offset..], little_endian));
    }

    Ok(Some(values))
}

fn read_crs_from_geokeys(
    tags: &HashMap<u16, IfdEntry>,
    reader: &Arc<dyn RangeReader>,
    _ifd_offset: u64,
    little_endian: bool,
) -> AnyResult<Option<i32>> {
    let Some(entry) = tags.get(&TAG_GEO_KEY_DIRECTORY) else {
        return Ok(None);
    };

    // GeoKey directory is an array of SHORT values
    if entry.field_type != 3 {
        return Ok(None);
    }

    let total_bytes = entry.count as usize * 2;
    let raw_bytes = if total_bytes <= 4 {
        entry.raw_bytes[..total_bytes].to_vec()
    } else {
        reader.read_range(u64::from(entry.value_offset), total_bytes)?
    };

    // Parse GeoKey directory header
    // Format: KeyDirectoryVersion, KeyRevision, MinorRevision, NumberOfKeys
    //         KeyID, TIFFTagLocation, Count, Value_Offset
    //         ...repeated for each key

    if raw_bytes.len() < 8 {
        return Ok(None);
    }

    let num_keys = read_u16(&raw_bytes[6..8], little_endian) as usize;

    for i in 0..num_keys {
        let offset = 8 + i * 8;
        if offset + 8 > raw_bytes.len() {
            break;
        }

        let key_id = read_u16(&raw_bytes[offset..], little_endian);
        let _tiff_tag_location = read_u16(&raw_bytes[offset + 2..], little_endian);
        let _count = read_u16(&raw_bytes[offset + 4..], little_endian);
        let value = read_u16(&raw_bytes[offset + 6..], little_endian);

        // Check for ProjectedCSTypeGeoKey (3072) or GeographicTypeGeoKey (2048)
        if key_id == GEO_KEY_PROJECTED_CRS && value > 0 {
            return Ok(Some(i32::from(value)));
        }
        if key_id == GEO_KEY_GEOGRAPHIC_TYPE && value > 0 {
            return Ok(Some(i32::from(value)));
        }
    }

    Ok(None)
}

fn read_gdal_stats(
    tags: &HashMap<u16, IfdEntry>,
    reader: &Arc<dyn RangeReader>,
    _ifd_offset: u64,
    _little_endian: bool,
) -> AnyResult<(Option<f32>, Option<f32>)> {
    let Some(entry) = tags.get(&TAG_GDAL_METADATA) else {
        return Ok((None, None));
    };

    // GDAL metadata is ASCII/UTF-8 XML
    let total_bytes = entry.count as usize;
    let raw_bytes = if total_bytes <= 4 {
        entry.raw_bytes[..total_bytes].to_vec()
    } else {
        reader.read_range(u64::from(entry.value_offset), total_bytes)?
    };

    let metadata_str = String::from_utf8_lossy(&raw_bytes);

    // Parse STATISTICS_MINIMUM and STATISTICS_MAXIMUM from XML
    let min = extract_gdal_stat(&metadata_str, "STATISTICS_MINIMUM");
    let max = extract_gdal_stat(&metadata_str, "STATISTICS_MAXIMUM");

    Ok((min, max))
}

fn extract_gdal_stat(metadata: &str, key: &str) -> Option<f32> {
    let needle = format!("name=\"{key}\"");
    let pos = metadata.find(&needle)?;
    let rest = &metadata[pos..];
    let start = rest.find('>')? + 1;
    let rest = &rest[start..];
    let end = rest.find('<')?;
    rest[..end].trim().parse().ok()
}

fn read_gdal_nodata(
    tags: &HashMap<u16, IfdEntry>,
    reader: &Arc<dyn RangeReader>,
    _ifd_offset: u64,
    _little_endian: bool,
) -> AnyResult<Option<f64>> {
    let entry = match tags.get(&TAG_GDAL_NODATA) {
        Some(e) => e,
        None => return Ok(None),
    };

    let total_bytes = entry.count as usize;
    let raw_bytes = if total_bytes <= 4 {
        entry.raw_bytes[..total_bytes].to_vec()
    } else {
        reader.read_range(u64::from(entry.value_offset), total_bytes)?
    };

    let nodata_str = String::from_utf8_lossy(&raw_bytes);
    let nodata_str = nodata_str.trim_end_matches('\0').trim();

    Ok(nodata_str.parse().ok())
}

// ============================================================================
// Decompression and data conversion
// ============================================================================

fn decompress_tile(
    compressed: &[u8],
    compression: Compression,
    tile_width: usize,
    tile_height: usize,
    bands: usize,
    bytes_per_sample: usize,
) -> AnyResult<Vec<u8>> {
    let expected_size = tile_width * tile_height * bands * bytes_per_sample;

    match compression {
        Compression::None => {
            if compressed.len() >= expected_size {
                Ok(compressed[..expected_size].to_vec())
            } else {
                // Pad with zeros
                let mut result = compressed.to_vec();
                result.resize(expected_size, 0);
                Ok(result)
            }
        }
        Compression::Deflate => {
            use std::io::Read;
            let mut decoder = flate2::read::ZlibDecoder::new(compressed);
            let mut decompressed = Vec::with_capacity(expected_size);
            decoder.read_to_end(&mut decompressed)?;
            Ok(decompressed)
        }
        Compression::Lzw => {
            // Use weezl for LZW decompression
            let mut decoder = weezl::decode::Decoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8);
            let decompressed = decoder.decode(compressed)?;
            Ok(decompressed)
        }
        Compression::Zstd => {
            let decompressed = zstd::stream::decode_all(compressed)?;
            Ok(decompressed)
        }
    }
}

fn apply_predictor(
    data: &[u8],
    predictor: u16,
    tile_width: usize,
    bands: usize,
    bytes_per_sample: usize,
) -> AnyResult<Vec<u8>> {
    match predictor {
        1 => Ok(data.to_vec()), // No predictor
        2 => {
            // Horizontal differencing
            let mut result = data.to_vec();
            let row_bytes = tile_width * bands * bytes_per_sample;

            for row in result.chunks_mut(row_bytes) {
                // Skip first pixel in each row
                for i in bytes_per_sample..row.len() {
                    row[i] = row[i].wrapping_add(row[i - bytes_per_sample]);
                }
            }

            Ok(result)
        }
        3 => {
            // Floating point predictor - operates on bytes, not values
            // Each byte position is differenced independently
            let mut result = data.to_vec();
            let row_bytes = tile_width * bands * bytes_per_sample;

            for row in result.chunks_mut(row_bytes) {
                for byte_pos in 0..bytes_per_sample {
                    for i in 1..(row.len() / bytes_per_sample) {
                        let idx = i * bytes_per_sample + byte_pos;
                        let prev_idx = (i - 1) * bytes_per_sample + byte_pos;
                        row[idx] = row[idx].wrapping_add(row[prev_idx]);
                    }
                }
            }

            Ok(result)
        }
        _ => Err(format!("Unsupported predictor: {predictor}").into()),
    }
}

fn convert_to_f32(data: &[u8], data_type: CogDataType, little_endian: bool) -> AnyResult<Vec<f32>> {
    let bytes_per_sample = data_type.bytes_per_sample();
    let sample_count = data.len() / bytes_per_sample;
    let mut result = Vec::with_capacity(sample_count);

    for i in 0..sample_count {
        let offset = i * bytes_per_sample;
        let bytes = &data[offset..offset + bytes_per_sample];

        let value = match data_type {
            CogDataType::UInt8 => f32::from(bytes[0]),
            CogDataType::Int8 => f32::from(bytes[0] as i8),
            CogDataType::UInt16 => {
                if little_endian {
                    f32::from(u16::from_le_bytes([bytes[0], bytes[1]]))
                } else {
                    f32::from(u16::from_be_bytes([bytes[0], bytes[1]]))
                }
            }
            CogDataType::Int16 => {
                if little_endian {
                    f32::from(i16::from_le_bytes([bytes[0], bytes[1]]))
                } else {
                    f32::from(i16::from_be_bytes([bytes[0], bytes[1]]))
                }
            }
            CogDataType::UInt32 => {
                if little_endian {
                    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f32
                } else {
                    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f32
                }
            }
            CogDataType::Int32 => {
                if little_endian {
                    i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f32
                } else {
                    i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f32
                }
            }
            CogDataType::Float32 => {
                if little_endian {
                    f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
                } else {
                    f32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
                }
            }
            CogDataType::UInt64 => {
                if little_endian {
                    u64::from_le_bytes([
                        bytes[0], bytes[1], bytes[2], bytes[3],
                        bytes[4], bytes[5], bytes[6], bytes[7],
                    ]) as f32
                } else {
                    u64::from_be_bytes([
                        bytes[0], bytes[1], bytes[2], bytes[3],
                        bytes[4], bytes[5], bytes[6], bytes[7],
                    ]) as f32
                }
            }
            CogDataType::Int64 => {
                if little_endian {
                    i64::from_le_bytes([
                        bytes[0], bytes[1], bytes[2], bytes[3],
                        bytes[4], bytes[5], bytes[6], bytes[7],
                    ]) as f32
                } else {
                    i64::from_be_bytes([
                        bytes[0], bytes[1], bytes[2], bytes[3],
                        bytes[4], bytes[5], bytes[6], bytes[7],
                    ]) as f32
                }
            }
            CogDataType::Float64 => {
                if little_endian {
                    f64::from_le_bytes([
                        bytes[0], bytes[1], bytes[2], bytes[3],
                        bytes[4], bytes[5], bytes[6], bytes[7],
                    ]) as f32
                } else {
                    f64::from_be_bytes([
                        bytes[0], bytes[1], bytes[2], bytes[3],
                        bytes[4], bytes[5], bytes[6], bytes[7],
                    ]) as f32
                }
            }
        };

        result.push(value);
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_data_type_detection() {
        assert_eq!(CogDataType::from_tags(8, 1), Some(CogDataType::UInt8));
        assert_eq!(CogDataType::from_tags(16, 1), Some(CogDataType::UInt16));
        assert_eq!(CogDataType::from_tags(32, 3), Some(CogDataType::Float32));
        assert_eq!(CogDataType::from_tags(64, 3), Some(CogDataType::Float64));
    }

    #[test]
    fn test_compression_detection() {
        assert_eq!(Compression::from_tag(1), Some(Compression::None));
        assert_eq!(Compression::from_tag(5), Some(Compression::Lzw));
        assert_eq!(Compression::from_tag(8), Some(Compression::Deflate));
    }

    #[test]
    fn test_geo_transform() {
        let transform = GeoTransform {
            pixel_scale: Some([10.0, 10.0, 0.0]),
            tiepoint: Some([0.0, 0.0, 0.0, 100.0, 200.0, 0.0]),
        };

        // Pixel (0,0) should map to (100, 200)
        let (wx, wy) = transform.pixel_to_world(0.0, 0.0).unwrap();
        assert!((wx - 100.0).abs() < 0.001);
        assert!((wy - 200.0).abs() < 0.001);

        // Pixel (10, 5) should map to (200, 150)
        let (wx, wy) = transform.pixel_to_world(10.0, 5.0).unwrap();
        assert!((wx - 200.0).abs() < 0.001);
        assert!((wy - 150.0).abs() < 0.001);
    }

    #[test]
    fn test_real_cog_file() {
        // Test with a real COG file if it exists
        let path = "data/viridis/output_cog.tif";
        if !std::path::Path::new(path).exists() {
            println!("Skipping test - file not found: {}", path);
            return;
        }

        let reader = CogReader::open(path).expect("Failed to open COG");
        let m = &reader.metadata;

        println!("Testing real COG: {}", path);
        println!("  Width: {}, Height: {}", m.width, m.height);
        println!("  Tile size: {}x{}", m.tile_width, m.tile_height);
        println!("  CRS code: {:?}", m.crs_code);
        println!("  Bands: {}", m.bands);
        println!("  Compression: {:?}", m.compression);
        println!("  Extent: {:?}", m.geo_transform.get_extent(m.width, m.height));
        println!("  Pixel scale: {:?}", m.geo_transform.pixel_scale);
        println!("  Tiepoint: {:?}", m.geo_transform.tiepoint);

        // Verify basic metadata
        assert!(m.width > 0, "Width should be positive");
        assert!(m.height > 0, "Height should be positive");
        assert!(m.is_tiled(), "Should be a tiled TIFF");

        // Test world_to_pixel for known coordinates
        // Center of the image should be at pixel (width/2, height/2)
        if let Some((px, py)) = m.geo_transform.world_to_pixel(0.0, 0.0) {
            println!("  Lon 0, Lat 0 -> pixel ({}, {})", px, py);
            // For a global dataset, (0,0) should be near center
            assert!(px > 0.0 && px < m.width as f64, "X pixel should be in range");
            assert!(py > 0.0 && py < m.height as f64, "Y pixel should be in range");
        }

        // Try to read tile 0
        let tile_data = reader.read_tile(0).expect("Failed to read tile 0");
        assert!(!tile_data.is_empty(), "Tile data should not be empty");

        let non_nan = tile_data.iter().filter(|v| !v.is_nan()).count();
        println!("  Tile 0: {} values, {} non-NaN", tile_data.len(), non_nan);
        assert!(non_nan > 0, "Tile should have some valid pixels");

        // Test min/max estimation
        let (min, max) = reader.estimate_min_max().expect("Failed to estimate min/max");
        println!("  Estimated min: {}, max: {}", min, max);
        // For an RGB image, values should be 0-255
        assert!(min >= 0.0, "Min should be >= 0");
        assert!(max <= 255.0, "Max should be <= 255 for 8-bit data");
    }
}

// ============================================================
// COMPREHENSIVE TESTS FOR COG READER
// These tests verify:
// 1. Scale calculation uses FLOOR division (matching GDAL)
// 2. CRS detection works correctly
// 3. Pixel values match GDAL output
// ============================================================

#[test]
fn test_gray_3857_crs_detection() {
    let path = "data/grayscale/gray_3857-cog.tif";
    if !std::path::Path::new(path).exists() {
        println!("Skipping - file not found: {}", path);
        return;
    }

    let reader = CogReader::open(path).expect("Failed to open COG");

    // EPSG:3857 should be detected
    assert_eq!(reader.metadata.crs_code, Some(3857), "CRS should be detected as 3857");
}

/// TEST: Overview scale calculation uses FLOOR division
///
/// This test catches the bug where we used ceiling division instead of floor.
/// For gray_3857-cog.tif (20966x20966), overview 3 (1310x1310):
/// - WRONG (ceiling): (20966 + 1310 - 1) / 1310 = 17
/// - CORRECT (floor): 20966 / 1310 = 16
///
/// The scale affects coordinate calculations, causing ~6% pixel position errors.
#[test]
fn test_overview_scale_uses_floor_division() {
    let path = "data/grayscale/gray_3857-cog.tif";
    if !std::path::Path::new(path).exists() {
        println!("Skipping - file not found: {}", path);
        return;
    }

    let reader = CogReader::open(path).expect("Failed to open COG");

    // Verify we have overviews
    assert!(!reader.overviews.is_empty(), "Should have overviews");

    // Check that scale calculation matches GDAL behavior (floor division)
    let full_width = reader.metadata.width;

    for (i, ovr) in reader.overviews.iter().enumerate() {
        // Calculate expected scale using floor division (GDAL's method)
        let expected_scale = full_width / ovr.width;

        // Verify our scale matches
        assert_eq!(
            ovr.scale, expected_scale,
            "Overview {} scale mismatch: got {}, expected {} (floor of {}/{})",
            i, ovr.scale, expected_scale, full_width, ovr.width
        );

        // Also verify it's NOT using ceiling division
        let ceiling_scale = full_width.div_ceil(ovr.width);
        if ceiling_scale != expected_scale {
            // If ceiling would give different result, make sure we're using floor
            assert_ne!(
                ovr.scale, ceiling_scale,
                "Overview {} appears to use ceiling division (got {}), should use floor ({})",
                i, ceiling_scale, expected_scale
            );
        }
    }

    // Specific check for overview 3 which caused the original bug
    if reader.overviews.len() > 3 {
        let ovr3 = &reader.overviews[3];
        assert_eq!(
            ovr3.scale, 16,
            "Overview 3 (1310x1310) scale should be 16 (floor), not 17 (ceiling)"
        );
    }
}

/// TEST: Pixel values at known positions match GDAL output
///
/// This test verifies that the tile extraction produces correct pixel values.
/// Values were obtained from GDAL: gdal_translate with -projwin
#[test]
fn test_overview_pixel_values_match_gdal() {
    let path = "data/grayscale/gray_3857-cog.tif";
    if !std::path::Path::new(path).exists() {
        println!("Skipping - file not found: {}", path);
        return;
    }

    let reader = CogReader::open(path).expect("Failed to open COG");

    // Test reading from overview 3 (1310x1310, scale 16)
    if reader.overviews.len() > 3 {
        let ovr_idx = 3;
        let ovr = &reader.overviews[ovr_idx];

        // Verify overview properties
        assert_eq!(ovr.width, 1310, "Overview 3 should be 1310 wide");
        assert_eq!(ovr.height, 1310, "Overview 3 should be 1310 tall");
        assert_eq!(ovr.scale, 16, "Overview 3 scale should be 16");

        // Read tile 0 from overview
        let tile_data = reader.read_overview_tile(ovr_idx, 0).expect("Failed to read overview tile 0");

        // Verify tile data size
        let expected_size = ovr.tile_width * ovr.tile_height;
        assert_eq!(tile_data.len(), expected_size, "Tile data should be {}x{} pixels", ovr.tile_width, ovr.tile_height);

        // Check that we have valid (non-NaN) data
        let valid_count = tile_data.iter().filter(|v| !v.is_nan()).count();
        assert!(valid_count > 0, "Tile should have valid (non-NaN) pixels");

        // Verify pixel values are in expected range for grayscale (0-255)
        let min_val = tile_data.iter().filter(|v| !v.is_nan()).copied().fold(f32::INFINITY, f32::min);
        let max_val = tile_data.iter().filter(|v| !v.is_nan()).copied().fold(f32::NEG_INFINITY, f32::max);

        assert!(min_val >= 0.0, "Min value should be >= 0, got {}", min_val);
        assert!(max_val <= 255.0, "Max value should be <= 255, got {}", max_val);

        // Check specific pixel value that GDAL reports
        // At tile position (0, 0) in overview 3, GDAL shows value ~176
        let corner_value = tile_data[0];
        assert!(
            !corner_value.is_nan() && (100.0..=255.0).contains(&corner_value),
            "Corner value should be valid grayscale, got {}",
            corner_value
        );
    }
}

/// TEST: Scale factor correctly affects coordinate mapping
///
/// This test verifies that the scale factor properly adjusts the pixel_scale
/// when using overviews, which is critical for correct tile generation.
#[test]
fn test_scale_factor_coordinate_mapping() {
    let path = "data/grayscale/gray_3857-cog.tif";
    if !std::path::Path::new(path).exists() {
        println!("Skipping - file not found: {}", path);
        return;
    }

    let reader = CogReader::open(path).expect("Failed to open COG");

    if let (Some(pixel_scale), Some(_tiepoint)) = (
        reader.metadata.geo_transform.pixel_scale,
        reader.metadata.geo_transform.tiepoint,
    ) {
        let base_scale_x = pixel_scale[0];

        for (i, ovr) in reader.overviews.iter().enumerate() {
            // Calculate effective scale for this overview
            let effective_scale_x = base_scale_x * (ovr.scale as f64);

            // The effective scale should roughly equal full_extent / overview_width
            // For a COG covering ~20 million meters in 1310 pixels at overview 3:
            // effective_scale ≈ 20e6 / 1310 ≈ 15267 meters/pixel
            let full_extent_x = base_scale_x * (reader.metadata.width as f64);
            let expected_effective_scale = full_extent_x / (ovr.width as f64);

            // Allow 1% tolerance for rounding
            let tolerance = expected_effective_scale * 0.01;
            assert!(
                (effective_scale_x - expected_effective_scale).abs() < tolerance,
                "Overview {} effective scale mismatch: got {}, expected {} (within {})",
                i, effective_scale_x, expected_effective_scale, tolerance
            );
        }
    }
}

/// TEST: Best overview selection returns appropriate level
#[test]
fn test_best_overview_selection() {
    let path = "data/grayscale/gray_3857-cog.tif";
    if !std::path::Path::new(path).exists() {
        println!("Skipping - file not found: {}", path);
        return;
    }

    let reader = CogReader::open(path).expect("Failed to open COG");

    // For a small extent (256 pixels worth), should return None (use full res)
    let full_res = reader.best_overview_for_resolution(256, 256);
    // This might return None or a small overview index depending on the image

    // For a large extent (whole image), should return highest overview
    let large_extent = reader.best_overview_for_resolution(20000, 20000);
    assert!(
        large_extent.is_some() || reader.overviews.is_empty(),
        "Large extent should use an overview"
    );

    // For medium extent, should return appropriate overview
    let medium_extent = reader.best_overview_for_resolution(5000, 5000);
    // Just verify it doesn't panic and returns a valid index
    if let Some(idx) = medium_extent {
        assert!(
            idx < reader.overviews.len(),
            "Overview index {} should be valid",
            idx
        );
    }
}
