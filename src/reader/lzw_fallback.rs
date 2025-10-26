//! Fallback LZW TIFF reader that streams tiles on demand.

use crate::reader::raster::RasterSource;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

type AnyResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const TAG_IMAGE_WIDTH: u16 = 256;
const TAG_IMAGE_LENGTH: u16 = 257;
const TAG_BITS_PER_SAMPLE: u16 = 258;
const TAG_COMPRESSION: u16 = 259;
const TAG_STRIP_OFFSETS: u16 = 273;
const TAG_SAMPLES_PER_PIXEL: u16 = 277;
const TAG_ROWS_PER_STRIP: u16 = 278;
const TAG_STRIP_BYTE_COUNTS: u16 = 279;
const TAG_PLANAR_CONFIGURATION: u16 = 284;
const TAG_PREDICTOR: u16 = 317;
const TAG_TILE_WIDTH: u16 = 322;
const TAG_TILE_LENGTH: u16 = 323;
const TAG_TILE_OFFSETS: u16 = 324;
const TAG_TILE_BYTE_COUNTS: u16 = 325;
const TAG_SAMPLE_FORMAT: u16 = 339;
const TAG_MODEL_PIXEL_SCALE: u16 = 33550;
const TAG_MODEL_TIEPOINT: u16 = 33922;

#[derive(Debug)]
struct TiffHeader {
    little_endian: bool,
    first_ifd_offset: u32,
}

#[derive(Debug)]
struct IfdEntry {
    tag: u16,
    field_type: u16,
    count: u32,
    value_offset: u32,
    raw_value_bytes: [u8; 4],
}

/// Streamed LZW raster source backed by on-demand tile decoding.
pub struct LzwRasterSource {
    path: PathBuf,
    image_width: usize,
    image_length: usize,
    tile_width: usize,
    tile_length: usize,
    tiles_across: usize,
    offsets: Vec<u64>,
    byte_counts: Vec<u32>,
    samples_per_pixel: usize,
    predictor: u32,
    bytes_per_sample: usize,
    is_tiled: bool,
    cache: Mutex<HashMap<usize, Arc<Vec<f32>>>>,
    model_pixel_scale: Option<[f64; 3]>,
    model_tiepoint: Option<[f64; 6]>,
}

impl LzwRasterSource {
    pub fn open(path: &PathBuf) -> AnyResult<Self> {
        let mut file = File::open(path)?;
        let header = read_tiff_header(&mut file)?;
        file.seek(SeekFrom::Start(header.first_ifd_offset as u64))?;
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
                "Unsupported BitsPerSample value {} (must be byte aligned)",
                bits_per_sample
            )
            .into());
        }

        let bytes_per_sample = bits_per_sample / 8;
        if bytes_per_sample != 1 {
            return Err(format!(
                "Unsupported bytes per sample {} (only 8-bit samples supported in fallback)",
                bytes_per_sample
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
                "Unsupported compression scheme {} (expected LZW)",
                compression
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
                "Unsupported predictor {} (only none and horizontal differencing supported)",
                predictor
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
                "Unsupported planar configuration {} (only chunky pixels supported)",
                planar_configuration
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
                "Unsupported sample format {} (only unsigned integer supported)",
                sample_format
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

        let (tile_width, tile_length, offsets, byte_counts, tiles_across, is_tiled) =
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

                let tiles_across = (image_width + tile_width - 1) / tile_width;
                let tiles_down = (image_length + tile_length - 1) / tile_length;

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

                let offsets_u64 = offsets.into_iter().map(|value| value as u64).collect();

                (
                    tile_width,
                    tile_length,
                    offsets_u64,
                    counts,
                    tiles_across,
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

                let offsets_u64 = strip_offsets
                    .into_iter()
                    .map(|value| value as u64)
                    .collect();
                (
                    image_width,
                    rows_per_strip.min(image_length),
                    offsets_u64,
                    strip_byte_counts,
                    1,
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

        Ok(Self {
            path: path.clone(),
            image_width,
            image_length,
            tile_width,
            tile_length,
            tiles_across,
            offsets,
            byte_counts,
            samples_per_pixel,
            predictor,
            bytes_per_sample,
            is_tiled,
            cache: Mutex::new(HashMap::new()),
            model_pixel_scale,
            model_tiepoint,
        })
    }

    pub fn pixel_scale(&self) -> Option<[f64; 3]> {
        self.model_pixel_scale
    }

    pub fn tiepoint(&self) -> Option<[f64; 6]> {
        self.model_tiepoint
    }

    pub fn compute_min_max(&self) -> AnyResult<(f32, f32)> {
        let mut min_value = f32::INFINITY;
        let mut max_value = f32::NEG_INFINITY;

        for tile_index in 0..self.offsets.len() {
            let tile_data = if let Some(cached) = self.cache.lock().unwrap().get(&tile_index) {
                cached.clone()
            } else {
                self.load_tile(tile_index)?
            };
            for &value in tile_data.iter() {
                if value.is_nan() {
                    continue;
                }
                if value < min_value {
                    min_value = value;
                }
                if value > max_value {
                    max_value = value;
                }
            }
        }

        if min_value == f32::INFINITY || max_value == f32::NEG_INFINITY {
            Ok((0.0, 0.0))
        } else {
            Ok((min_value, max_value))
        }
    }

    fn fetch_tile(&self, tile_index: usize) -> AnyResult<Arc<Vec<f32>>> {
        if let Some(cached) = self.cache.lock().unwrap().get(&tile_index) {
            return Ok(cached.clone());
        }

        let arc = self.load_tile(tile_index)?;
        self.cache.lock().unwrap().insert(tile_index, arc.clone());
        Ok(arc)
    }

    fn load_tile(&self, tile_index: usize) -> AnyResult<Arc<Vec<f32>>> {
        if tile_index >= self.offsets.len() {
            return Err(format!("Tile index {} out of range", tile_index).into());
        }

        let offset = self.offsets[tile_index];
        let byte_count = self.byte_counts[tile_index] as usize;

        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut compressed_data = vec![0u8; byte_count];
        file.read_exact(&mut compressed_data)?;

        let expected_tile_bytes = self
            .tile_width
            .checked_mul(self.tile_length)
            .and_then(|value| value.checked_mul(self.samples_per_pixel))
            .and_then(|value| value.checked_mul(self.bytes_per_sample))
            .ok_or_else(|| "Tile byte size overflow".to_string())?;

        let (mut decompressed, actual_bytes) =
            try_lzw_decompress(&compressed_data, expected_tile_bytes)?;

        if self.predictor == 2 {
            apply_horizontal_predictor_u8(
                &mut decompressed,
                self.tile_width,
                self.tile_length,
                self.samples_per_pixel,
            );
        }

        let mut values =
            vec![f32::NAN; self.tile_width * self.tile_length * self.samples_per_pixel];
        let valid_samples = actual_bytes.min(decompressed.len());
        for idx in 0..valid_samples {
            values[idx] = decompressed[idx] as f32;
        }

        Ok(Arc::new(values))
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

fn read_tiff_header(file: &mut File) -> AnyResult<TiffHeader> {
    let mut header_bytes = [0u8; 8];
    file.read_exact(&mut header_bytes)?;

    let little_endian = match &header_bytes[0..2] {
        b"II" => true,
        b"MM" => false,
        _ => return Err("Invalid TIFF signature".into()),
    };

    let version = if little_endian {
        u16::from_le_bytes([header_bytes[2], header_bytes[3]])
    } else {
        u16::from_be_bytes([header_bytes[2], header_bytes[3]])
    };

    if version != 42 {
        return Err("Invalid TIFF version".into());
    }

    let first_ifd_offset = if little_endian {
        u32::from_le_bytes([
            header_bytes[4],
            header_bytes[5],
            header_bytes[6],
            header_bytes[7],
        ])
    } else {
        u32::from_be_bytes([
            header_bytes[4],
            header_bytes[5],
            header_bytes[6],
            header_bytes[7],
        ])
    };

    Ok(TiffHeader {
        little_endian,
        first_ifd_offset,
    })
}

fn read_ifd(file: &mut File, little_endian: bool) -> AnyResult<Vec<IfdEntry>> {
    let mut entry_count_bytes = [0u8; 2];
    file.read_exact(&mut entry_count_bytes)?;
    let entry_count = if little_endian {
        u16::from_le_bytes(entry_count_bytes)
    } else {
        u16::from_be_bytes(entry_count_bytes)
    };

    let mut entries = Vec::with_capacity(entry_count as usize);

    for _ in 0..entry_count {
        let mut entry_bytes = [0u8; 12];
        file.read_exact(&mut entry_bytes)?;

        let tag = if little_endian {
            u16::from_le_bytes([entry_bytes[0], entry_bytes[1]])
        } else {
            u16::from_be_bytes([entry_bytes[0], entry_bytes[1]])
        };

        let field_type = if little_endian {
            u16::from_le_bytes([entry_bytes[2], entry_bytes[3]])
        } else {
            u16::from_be_bytes([entry_bytes[2], entry_bytes[3]])
        };

        let count = if little_endian {
            u32::from_le_bytes([
                entry_bytes[4],
                entry_bytes[5],
                entry_bytes[6],
                entry_bytes[7],
            ])
        } else {
            u32::from_be_bytes([
                entry_bytes[4],
                entry_bytes[5],
                entry_bytes[6],
                entry_bytes[7],
            ])
        };

        let raw_value_bytes = [
            entry_bytes[8],
            entry_bytes[9],
            entry_bytes[10],
            entry_bytes[11],
        ];

        let value_offset = if little_endian {
            u32::from_le_bytes(raw_value_bytes)
        } else {
            u32::from_be_bytes(raw_value_bytes)
        };

        entries.push(IfdEntry {
            tag,
            field_type,
            count,
            value_offset,
            raw_value_bytes,
        });
    }

    Ok(entries)
}

fn get_entry<'a>(entries: &'a [IfdEntry], tag: u16) -> Option<&'a IfdEntry> {
    entries.iter().find(|entry| entry.tag == tag)
}

fn read_entry_values_u32(
    file: &mut File,
    entry: &IfdEntry,
    little_endian: bool,
) -> AnyResult<Vec<u32>> {
    let type_size = match entry.field_type {
        3 => 2,
        4 => 4,
        _ => {
            return Err(format!(
                "Unsupported field type {} for u32 conversion",
                entry.field_type
            )
            .into());
        }
    };

    let total_bytes = entry.count as usize * type_size;
    let mut raw_bytes = vec![0u8; total_bytes];

    if total_bytes <= 4 {
        if little_endian {
            raw_bytes.copy_from_slice(&entry.raw_value_bytes[..total_bytes]);
        } else {
            let start = 4 - total_bytes;
            raw_bytes.copy_from_slice(&entry.raw_value_bytes[start..]);
        }
    } else {
        let current_pos = file.stream_position()?;
        file.seek(SeekFrom::Start(entry.value_offset as u64))?;
        file.read_exact(&mut raw_bytes)?;
        file.seek(SeekFrom::Start(current_pos))?;
    }

    let mut values = Vec::with_capacity(entry.count as usize);
    for chunk in raw_bytes.chunks_exact(type_size) {
        let value = match entry.field_type {
            3 => {
                if little_endian {
                    u16::from_le_bytes([chunk[0], chunk[1]]) as u32
                } else {
                    u16::from_be_bytes([chunk[0], chunk[1]]) as u32
                }
            }
            4 => {
                if little_endian {
                    u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])
                } else {
                    u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])
                }
            }
            _ => unreachable!(),
        };
        values.push(value);
    }

    Ok(values)
}

fn read_entry_values_f64(
    file: &mut File,
    entry: &IfdEntry,
    little_endian: bool,
) -> AnyResult<Vec<f64>> {
    if entry.field_type != 12 {
        return Err(format!(
            "Unsupported field type {} for f64 conversion",
            entry.field_type
        )
        .into());
    }

    let total_bytes = entry.count as usize * 8;
    let mut raw_bytes = vec![0u8; total_bytes];

    if total_bytes <= 4 {
        return Err("Unexpected inline storage for double precision data".into());
    }

    let current_pos = file.stream_position()?;
    file.seek(SeekFrom::Start(entry.value_offset as u64))?;
    file.read_exact(&mut raw_bytes)?;
    file.seek(SeekFrom::Start(current_pos))?;

    let mut values = Vec::with_capacity(entry.count as usize);
    for chunk in raw_bytes.chunks_exact(8) {
        let value = if little_endian {
            f64::from_le_bytes([
                chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6], chunk[7],
            ])
        } else {
            f64::from_be_bytes([
                chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6], chunk[7],
            ])
        };
        values.push(value);
    }

    Ok(values)
}

fn read_tag_u32(
    file: &mut File,
    entries: &[IfdEntry],
    tag: u16,
    little_endian: bool,
) -> AnyResult<u32> {
    let entry = get_entry(entries, tag).ok_or_else(|| format!("Tag {} not found", tag))?;
    let values = read_entry_values_u32(file, entry, little_endian)?;
    values
        .get(0)
        .copied()
        .ok_or_else(|| format!("Tag {} missing value", tag).into())
}

fn read_tag_u32_vec(
    file: &mut File,
    entries: &[IfdEntry],
    tag: u16,
    little_endian: bool,
) -> AnyResult<Vec<u32>> {
    let entry = get_entry(entries, tag).ok_or_else(|| format!("Tag {} not found", tag))?;
    read_entry_values_u32(file, entry, little_endian)
}

fn read_tag_u32_vec_optional(
    file: &mut File,
    entries: &[IfdEntry],
    tag: u16,
    little_endian: bool,
) -> AnyResult<Option<Vec<u32>>> {
    Ok(match get_entry(entries, tag) {
        Some(entry) => Some(read_entry_values_u32(file, entry, little_endian)?),
        None => None,
    })
}

fn read_tag_f64_triplet(
    file: &mut File,
    entries: &[IfdEntry],
    tag: u16,
    little_endian: bool,
) -> AnyResult<Option<[f64; 3]>> {
    let entry = match get_entry(entries, tag) {
        Some(entry) => entry,
        None => return Ok(None),
    };

    let values = read_entry_values_f64(file, entry, little_endian)?;
    if values.len() >= 3 {
        Ok(Some([values[0], values[1], values[2]]))
    } else {
        Ok(None)
    }
}

fn read_tag_f64_six(
    file: &mut File,
    entries: &[IfdEntry],
    tag: u16,
    little_endian: bool,
) -> AnyResult<Option<[f64; 6]>> {
    let entry = match get_entry(entries, tag) {
        Some(entry) => entry,
        None => return Ok(None),
    };

    let values = read_entry_values_f64(file, entry, little_endian)?;
    if values.len() >= 6 {
        Ok(Some([
            values[0], values[1], values[2], values[3], values[4], values[5],
        ]))
    } else {
        Ok(None)
    }
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
