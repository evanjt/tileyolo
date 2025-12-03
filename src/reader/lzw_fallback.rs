//! Fallback LZW TIFF reader that streams tiles on demand.

use crate::reader::raster::RasterSource;
use crate::reader::tiff_utils::{
    AnyResult, TAG_BITS_PER_SAMPLE, TAG_COMPRESSION, TAG_GDAL_METADATA, TAG_GDAL_NODATA,
    TAG_IMAGE_LENGTH, TAG_IMAGE_WIDTH, TAG_MODEL_PIXEL_SCALE, TAG_MODEL_TIEPOINT,
    TAG_PLANAR_CONFIGURATION, TAG_PREDICTOR, TAG_ROWS_PER_STRIP, TAG_SAMPLE_FORMAT,
    TAG_SAMPLES_PER_PIXEL, TAG_STRIP_BYTE_COUNTS, TAG_STRIP_OFFSETS, TAG_TILE_BYTE_COUNTS,
    TAG_TILE_LENGTH, TAG_TILE_OFFSETS, TAG_TILE_WIDTH, parse_gdal_metadata_stats, read_ifd,
    read_tag_f64_six, read_tag_f64_triplet, read_tag_string_from_ifd, read_tag_u32,
    read_tag_u32_vec, read_tag_u32_vec_optional, read_tiff_header,
};
use crate::reader::tile_cache::{self, TileKind};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Streamed LZW raster source backed by on-demand tile decoding.
pub struct LzwRasterSource {
    path: PathBuf,
    image_width: usize,
    image_length: usize,
    tile_width: usize,
    tile_length: usize,
    tiles_across: usize,
    tiles_down: usize,
    offsets: Arc<Vec<u64>>,
    byte_counts: Arc<Vec<u32>>,
    samples_per_pixel: usize,
    predictor: u32,
    bytes_per_sample: usize,
    is_tiled: bool,
    model_pixel_scale: Option<[f64; 3]>,
    model_tiepoint: Option<[f64; 6]>,
    nodata: Option<f64>,
    min_max: Mutex<Option<(f32, f32)>>,
}

impl LzwRasterSource {
    pub fn open(path: &PathBuf) -> AnyResult<Self> {
        let mut file = File::open(path)?;
        let header = read_tiff_header(&mut file)?;
        file.seek(SeekFrom::Start(u64::from(header.first_ifd_offset)))?;
        let ifd_entries = read_ifd(&mut file, header.little_endian)?;

        let image_width = read_tag_u32(
            &mut file,
            &ifd_entries,
            TAG_IMAGE_WIDTH,
            header.little_endian,
        )? as usize;
        let image_length = read_tag_u32(
            &mut file,
            &ifd_entries,
            TAG_IMAGE_LENGTH,
            header.little_endian,
        )? as usize;

        let bits_per_sample = read_tag_u32_vec_optional(
            &mut file,
            &ifd_entries,
            TAG_BITS_PER_SAMPLE,
            header.little_endian,
        )?
        .as_ref()
        .and_then(|values| values.first().copied())
        .unwrap_or(8) as usize;

        if bits_per_sample % 8 != 0 {
            return Err(format!(
                "Unsupported BitsPerSample value {bits_per_sample} (must be byte aligned)"
            )
            .into());
        }

        let bytes_per_sample = bits_per_sample / 8;
        if bytes_per_sample != 1 {
            return Err(format!(
                "Unsupported bytes per sample {bytes_per_sample} (only 8-bit samples supported in fallback)"
            )
            .into());
        }

        let samples_per_pixel = read_tag_u32_vec_optional(
            &mut file,
            &ifd_entries,
            TAG_SAMPLES_PER_PIXEL,
            header.little_endian,
        )?
        .as_ref()
        .and_then(|values| values.first().copied())
        .unwrap_or(1) as usize;

        let compression = read_tag_u32(
            &mut file,
            &ifd_entries,
            TAG_COMPRESSION,
            header.little_endian,
        )?;
        if compression != 5 {
            return Err(format!(
                "Unsupported compression scheme {compression} (expected LZW)"
            )
            .into());
        }

        let predictor = read_tag_u32_vec_optional(
            &mut file,
            &ifd_entries,
            TAG_PREDICTOR,
            header.little_endian,
        )?
        .as_ref()
        .and_then(|values| values.first().copied())
        .unwrap_or(1) as u32;
        if predictor != 1 && predictor != 2 {
            return Err(format!(
                "Unsupported predictor {predictor} (only none and horizontal differencing supported)"
            )
            .into());
        }

        let planar_configuration = read_tag_u32_vec_optional(
            &mut file,
            &ifd_entries,
            TAG_PLANAR_CONFIGURATION,
            header.little_endian,
        )?
        .as_ref()
        .and_then(|values| values.first().copied())
        .unwrap_or(1);
        if planar_configuration != 1 {
            return Err(format!(
                "Unsupported planar configuration {planar_configuration} (only chunky pixels supported)"
            )
            .into());
        }

        let sample_format = read_tag_u32_vec_optional(
            &mut file,
            &ifd_entries,
            TAG_SAMPLE_FORMAT,
            header.little_endian,
        )?
        .as_ref()
        .and_then(|values| values.first().copied())
        .unwrap_or(1);
        if sample_format != 1 {
            return Err(format!(
                "Unsupported sample format {sample_format} (only unsigned integer supported)"
            )
            .into());
        }

        let tile_offsets = read_tag_u32_vec_optional(
            &mut file,
            &ifd_entries,
            TAG_TILE_OFFSETS,
            header.little_endian,
        )?;
        let tile_byte_counts = read_tag_u32_vec_optional(
            &mut file,
            &ifd_entries,
            TAG_TILE_BYTE_COUNTS,
            header.little_endian,
        )?;

        let (tile_width, tile_length, offsets, byte_counts, tiles_across, tiles_down, is_tiled) =
            if let (Some(offsets), Some(counts)) = (tile_offsets, tile_byte_counts) {
                let tile_width = read_tag_u32(
                    &mut file,
                    &ifd_entries,
                    TAG_TILE_WIDTH,
                    header.little_endian,
                )? as usize;
                let tile_length = read_tag_u32(
                    &mut file,
                    &ifd_entries,
                    TAG_TILE_LENGTH,
                    header.little_endian,
                )? as usize;

                let tiles_across = image_width.div_ceil(tile_width);
                let tiles_down = image_length.div_ceil(tile_length);

                if offsets.len() != counts.len() {
                    return Err("TileOffsets and TileByteCounts length mismatch".into());
                }

                if offsets.len() < tiles_across * tiles_down {
                    return Err(format!(
                        "Not enough tile records: have {}, expected {} ({} across × {} down)",
                        offsets.len(),
                        tiles_across * tiles_down,
                        tiles_across,
                        tiles_down
                    )
                    .into());
                }

                let offsets_u64: Arc<Vec<u64>> =
                    Arc::new(offsets.into_iter().map(u64::from).collect());
                let counts_arc: Arc<Vec<u32>> = Arc::new(counts);

                (
                    tile_width,
                    tile_length,
                    offsets_u64,
                    counts_arc,
                    tiles_across,
                    tiles_down,
                    true,
                )
            } else {
                let strip_offsets = read_tag_u32_vec(
                    &mut file,
                    &ifd_entries,
                    TAG_STRIP_OFFSETS,
                    header.little_endian,
                )?;
                let strip_byte_counts = read_tag_u32_vec(
                    &mut file,
                    &ifd_entries,
                    TAG_STRIP_BYTE_COUNTS,
                    header.little_endian,
                )?;

                if strip_offsets.len() != strip_byte_counts.len() {
                    return Err("StripOffsets and StripByteCounts length mismatch".into());
                }

                let rows_per_strip = read_tag_u32(
                    &mut file,
                    &ifd_entries,
                    TAG_ROWS_PER_STRIP,
                    header.little_endian,
                )? as usize;

                let offsets_u64: Arc<Vec<u64>> = Arc::new(
                    strip_offsets
                        .into_iter()
                        .map(u64::from)
                        .collect(),
                );
                let counts_arc: Arc<Vec<u32>> = Arc::new(strip_byte_counts);
                let tiles_down = counts_arc.len();
                (
                    image_width,
                    rows_per_strip.min(image_length),
                    offsets_u64,
                    counts_arc,
                    1,
                    tiles_down,
                    false,
                )
            };

        if tile_width == 0 || tile_length == 0 {
            return Err("Invalid tile dimensions detected (width/length must be non-zero)".into());
        }

        let model_pixel_scale = read_tag_f64_triplet(
            &mut file,
            &ifd_entries,
            TAG_MODEL_PIXEL_SCALE,
            header.little_endian,
        )?;
        let model_tiepoint = read_tag_f64_six(
            &mut file,
            &ifd_entries,
            TAG_MODEL_TIEPOINT,
            header.little_endian,
        )?;

        let metadata_stats = read_tag_string_from_ifd(
            &mut file,
            &ifd_entries,
            header.little_endian,
            TAG_GDAL_METADATA,
        )
        .ok()
        .and_then(|s| parse_gdal_metadata_stats(&s));

        // Read GDAL_NODATA tag (tag 42113) - stored as ASCII string
        let nodata = read_tag_string_from_ifd(
            &mut file,
            &ifd_entries,
            header.little_endian,
            TAG_GDAL_NODATA,
        )
        .ok()
        .and_then(|s| s.trim_end_matches('\0').trim().parse::<f64>().ok());

        Ok(Self {
            path: path.clone(),
            image_width,
            image_length,
            tile_width,
            tile_length,
            tiles_across,
            tiles_down,
            offsets,
            byte_counts,
            samples_per_pixel,
            predictor,
            bytes_per_sample,
            is_tiled,
            model_pixel_scale,
            model_tiepoint,
            nodata,
            min_max: Mutex::new(metadata_stats),
        })
    }

    pub fn pixel_scale(&self) -> Option<[f64; 3]> {
        self.model_pixel_scale
    }

    pub fn tiepoint(&self) -> Option<[f64; 6]> {
        self.model_tiepoint
    }

    pub fn compute_min_max(&self) -> AnyResult<(f32, f32)> {
        if let Some(result) = *self.min_max.lock().unwrap() {
            return Ok(result);
        }

        let mut min_value = f32::INFINITY;
        let mut max_value = f32::NEG_INFINITY;
        let nodata_f32 = self.nodata.map(|v| v as f32);

        for tile_index in 0..self.offsets.len() {
            let tile_data = self.fetch_tile(tile_index)?;
            for &value in tile_data.iter() {
                // Skip NaN values
                if value.is_nan() {
                    continue;
                }
                // Skip NoData values
                if let Some(nd) = nodata_f32 {
                    if (value - nd).abs() < f32::EPSILON {
                        continue;
                    }
                }
                if value < min_value {
                    min_value = value;
                }
                if value > max_value {
                    max_value = value;
                }
            }
        }

        let result = if min_value == f32::INFINITY || max_value == f32::NEG_INFINITY {
            (0.0, 0.0)
        } else {
            (min_value, max_value)
        };

        *self.min_max.lock().unwrap() = Some(result);
        Ok(result)
    }

    fn fetch_tile(&self, tile_index: usize) -> AnyResult<Arc<Vec<f32>>> {
        if let Some(cached) = tile_cache::get(&self.path, TileKind::Lzw, tile_index) {
            return Ok(cached);
        }

        let tile = self.load_tile(tile_index)?;
        tile_cache::insert(&self.path, TileKind::Lzw, tile_index, Arc::clone(&tile));
        self.prefetch_neighbors(tile_index);
        Ok(tile)
    }

    fn load_tile(&self, tile_index: usize) -> AnyResult<Arc<Vec<f32>>> {
        decompress_lzw_tile(
            self.path.as_path(),
            &self.offsets,
            &self.byte_counts,
            self.tile_width,
            self.tile_length,
            self.samples_per_pixel,
            self.predictor,
            self.bytes_per_sample,
            tile_index,
        )
    }

    fn neighbor_indices(&self, tile_index: usize) -> Vec<usize> {
        let tile_col = tile_index % self.tiles_across;
        let tile_row = tile_index / self.tiles_across;
        let mut neighbors = Vec::with_capacity(4);

        if tile_col > 0 {
            neighbors.push(tile_index - 1);
        }
        if tile_col + 1 < self.tiles_across {
            neighbors.push(tile_index + 1);
        }
        if tile_row > 0 {
            neighbors.push(tile_index - self.tiles_across);
        }
        if tile_row + 1 < self.tiles_down {
            neighbors.push(tile_index + self.tiles_across);
        }

        neighbors
    }

    fn prefetch_neighbors(&self, tile_index: usize) {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let neighbors = self.neighbor_indices(tile_index);
            let config = LzwPrefetchConfig {
                path: self.path.clone(),
                offsets: Arc::clone(&self.offsets),
                byte_counts: Arc::clone(&self.byte_counts),
                tile_width: self.tile_width,
                tile_length: self.tile_length,
                samples_per_pixel: self.samples_per_pixel,
                predictor: self.predictor,
                bytes_per_sample: self.bytes_per_sample,
            };

            for neighbor in neighbors {
                if tile_cache::contains(&config.path, TileKind::Lzw, neighbor) {
                    continue;
                }

                let cfg = config.clone();
                handle.spawn_blocking(move || {
                    if let Ok(tile) = decompress_lzw_tile(
                        cfg.path.as_path(),
                        cfg.offsets.as_ref(),
                        cfg.byte_counts.as_ref(),
                        cfg.tile_width,
                        cfg.tile_length,
                        cfg.samples_per_pixel,
                        cfg.predictor,
                        cfg.bytes_per_sample,
                        neighbor,
                    ) {
                        tile_cache::insert(&cfg.path, TileKind::Lzw, neighbor, tile);
                    }
                });
            }
        }
    }

    fn tile_dimensions(&self, tile_index: usize) -> (usize, usize) {
        if self.is_tiled {
            (self.tile_width, self.tile_length)
        } else {
            let remaining_rows = self
                .image_length
                .saturating_sub(tile_index * self.tile_length);
            (self.tile_width, remaining_rows.min(self.tile_length))
        }
    }
}

impl RasterSource for LzwRasterSource {
    fn bands(&self) -> usize {
        self.samples_per_pixel
    }

    fn width(&self) -> usize {
        self.image_width
    }

    fn height(&self) -> usize {
        self.image_length
    }

    fn sample(&self, band: usize, x: usize, y: usize) -> Option<f32> {
        if band >= self.samples_per_pixel || x >= self.image_width || y >= self.image_length {
            return None;
        }

        let tile_x = x / self.tile_width;
        let tile_y = y / self.tile_length;
        let tile_index = tile_y * self.tiles_across + tile_x;

        let within_x = x % self.tile_width;
        let within_y = y % self.tile_length;

        let (tile_width, tile_height) = self.tile_dimensions(tile_index);
        if within_x >= tile_width || within_y >= tile_height {
            return None;
        }

        let tile = self.fetch_tile(tile_index).ok()?;
        let pixel_index = within_y * self.tile_width * self.samples_per_pixel
            + within_x * self.samples_per_pixel
            + band;
        tile.get(pixel_index).copied()
    }
}

pub fn try_read_lzw_tiff_fallback(path: &PathBuf) -> AnyResult<LzwRasterSource> {
    LzwRasterSource::open(path)
}

fn try_lzw_decompress(compressed: &[u8], expected_bytes: usize) -> AnyResult<(Vec<u8>, usize)> {
    let mut decoder = weezl::decode::Decoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8);
    match decoder.decode(compressed) {
        Ok(mut bytes) => {
            let original_len = bytes.len();
            if bytes.len() < expected_bytes {
                bytes.resize(expected_bytes, 0);
            } else if bytes.len() > expected_bytes {
                bytes.truncate(expected_bytes);
            }
            Ok((bytes, original_len))
        }
        Err(err) => Err(format!("LZW decode failure: {err}").into()),
    }
}

fn apply_horizontal_predictor_u8(
    data: &mut [u8],
    tile_width: usize,
    tile_length: usize,
    samples_per_pixel: usize,
) {
    if samples_per_pixel == 0 || tile_width == 0 {
        return;
    }

    let stride = tile_width * samples_per_pixel;

    for row in 0..tile_length {
        let row_start = row * stride;
        for col in 1..tile_width {
            let current_base = row_start + col * samples_per_pixel;
            let previous_base = current_base - samples_per_pixel;

            for sample in 0..samples_per_pixel {
                let idx = current_base + sample;
                let prev_idx = previous_base + sample;
                if idx < data.len() && prev_idx < data.len() {
                    data[idx] = data[idx].wrapping_add(data[prev_idx]);
                }
            }
        }
    }
}

#[derive(Clone)]
struct LzwPrefetchConfig {
    path: PathBuf,
    offsets: Arc<Vec<u64>>,
    byte_counts: Arc<Vec<u32>>,
    tile_width: usize,
    tile_length: usize,
    samples_per_pixel: usize,
    predictor: u32,
    bytes_per_sample: usize,
}

fn decompress_lzw_tile(
    path: &Path,
    offsets: &[u64],
    byte_counts: &[u32],
    tile_width: usize,
    tile_length: usize,
    samples_per_pixel: usize,
    predictor: u32,
    bytes_per_sample: usize,
    tile_index: usize,
) -> AnyResult<Arc<Vec<f32>>> {
    if tile_index >= offsets.len() {
        return Err(format!("Tile index {tile_index} out of range").into());
    }

    let offset = offsets[tile_index];
    let byte_count = byte_counts[tile_index] as usize;

    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut compressed_data = vec![0u8; byte_count];
    file.read_exact(&mut compressed_data)?;

    let expected_tile_bytes = tile_width
        .checked_mul(tile_length)
        .and_then(|value| value.checked_mul(samples_per_pixel))
        .and_then(|value| value.checked_mul(bytes_per_sample))
        .ok_or_else(|| "Tile byte size overflow".to_string())?;

    let (mut decompressed, actual_bytes) =
        try_lzw_decompress(&compressed_data, expected_tile_bytes)?;

    if predictor == 2 {
        apply_horizontal_predictor_u8(
            &mut decompressed,
            tile_width,
            tile_length,
            samples_per_pixel,
        );
    }

    let mut values = vec![f32::NAN; tile_width * tile_length * samples_per_pixel];
    let valid_samples = actual_bytes.min(decompressed.len()).min(values.len());
    for idx in 0..valid_samples {
        values[idx] = f32::from(decompressed[idx]);
    }

    Ok(Arc::new(values))
}
