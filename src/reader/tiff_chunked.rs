use crate::reader::raster::RasterSource;
use crate::reader::tiff_utils::{
    AnyResult, TAG_GDAL_METADATA, parse_gdal_metadata_stats, read_ifd, read_tag_string_from_ifd,
    read_tiff_header,
};
use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tiff::decoder::{Decoder, DecodingResult, Limits};
use tiff::tags::Tag;

struct ChunkCacheEntry {
    data: Vec<f32>,
}

pub struct TiffChunkedRasterSource {
    path: PathBuf,
    image_width: usize,
    image_length: usize,
    chunk_width: usize,
    chunk_height: usize,
    chunks_across: usize,
    chunks_down: usize,
    samples_per_pixel: usize,
    cache: Mutex<HashMap<usize, Arc<ChunkCacheEntry>>>,
    pixel_scale: Option<[f64; 3]>,
    tiepoint: Option<[f64; 6]>,
    min_max: Mutex<Option<(f32, f32)>>,
}

impl TiffChunkedRasterSource {
    pub fn open(path: &PathBuf) -> AnyResult<Self> {
        let mut decoder = Decoder::new(File::open(path)?)?;
        decoder = decoder.with_limits(Limits::unlimited());

        let (width_u32, height_u32) = decoder.dimensions()?;
        let width = width_u32 as usize;
        let height = height_u32 as usize;

        let color_type = decoder.colortype()?;
        let samples_per_pixel = match color_type {
            tiff::ColorType::Gray(_) => 1,
            _ => {
                return Err(
                    "Unsupported color type for chunked reader (only grayscale supported)".into(),
                );
            }
        };

        let (chunk_width_u32, chunk_height_u32) = decoder.chunk_dimensions();
        let chunk_width = chunk_width_u32.max(1) as usize;
        let chunk_height = chunk_height_u32.max(1) as usize;

        let chunks_across = (width + chunk_width - 1) / chunk_width;
        let chunks_down = (height + chunk_height - 1) / chunk_height;

        let pixel_scale = decoder
            .get_tag_f64_vec(Tag::ModelPixelScaleTag)
            .ok()
            .and_then(|v| {
                if v.len() >= 3 {
                    Some([v[0], v[1], v[2]])
                } else {
                    None
                }
            });

        let tiepoint = decoder
            .get_tag_f64_vec(Tag::ModelTiepointTag)
            .ok()
            .and_then(|v| {
                if v.len() >= 6 {
                    Some([v[0], v[1], v[2], v[3], v[4], v[5]])
                } else {
                    None
                }
            });

        let gdal_metadata = decoder
            .get_tag_ascii_string(Tag::from_u16_exhaustive(TAG_GDAL_METADATA))
            .ok();

        drop(decoder);

        let mut min_max_hint = gdal_metadata
            .as_ref()
            .and_then(|s| parse_gdal_metadata_stats(s));

        if min_max_hint.is_none() {
            // fallback to reading via low-level utils to attempt metadata parse
            if let Ok(mut file) = File::open(path) {
                if let Ok(header) = read_tiff_header(&mut file) {
                    if let Ok(ifd_entries) = read_ifd(&mut file, header.little_endian) {
                        if let Ok(meta_string) = read_tag_string_from_ifd(
                            &mut file,
                            &ifd_entries,
                            header.little_endian,
                            TAG_GDAL_METADATA,
                        ) {
                            min_max_hint = parse_gdal_metadata_stats(&meta_string);
                        }
                    }
                }
            }
        }

        Ok(Self {
            path: path.clone(),
            image_width: width,
            image_length: height,
            chunk_width,
            chunk_height,
            chunks_across,
            chunks_down,
            samples_per_pixel,
            cache: Mutex::new(HashMap::new()),
            pixel_scale,
            tiepoint,
            min_max: Mutex::new(min_max_hint),
        })
    }

    fn chunk_dimensions_for_index(&self, chunk_index: usize) -> (usize, usize) {
        let chunk_col = chunk_index % self.chunks_across;
        let chunk_row = chunk_index / self.chunks_across;

        let width = if chunk_col == self.chunks_across - 1 {
            self.image_width - chunk_col * self.chunk_width
        } else {
            self.chunk_width
        };

        let height = if chunk_row == self.chunks_down - 1 {
            self.image_length - chunk_row * self.chunk_height
        } else {
            self.chunk_height
        };

        (width, height)
    }

    fn open_decoder(&self) -> AnyResult<Decoder<File>> {
        let decoder = Decoder::new(File::open(&self.path)?)?;
        Ok(decoder.with_limits(Limits::unlimited()))
    }

    fn load_chunk(&self, chunk_index: usize) -> AnyResult<Arc<ChunkCacheEntry>> {
        let mut decoder = self.open_decoder()?;
        let data_dims = decoder.chunk_data_dimensions(chunk_index as u32);
        let actual_width = data_dims.0 as usize;
        let actual_height = data_dims.1 as usize;

        let decoded = decoder.read_chunk(chunk_index as u32)?;
        drop(decoder);

        let values = convert_decoding_result(decoded);
        let expected_len = actual_width * actual_height * self.samples_per_pixel;
        if values.len() != expected_len {
            return Err(format!(
                "Decoded chunk has unexpected length {} (expected {})",
                values.len(),
                expected_len
            )
            .into());
        }

        let mut padded =
            vec![f32::NAN; self.chunk_width * self.chunk_height * self.samples_per_pixel];

        for row in 0..actual_height {
            for col in 0..actual_width {
                let base_src = (row * actual_width + col) * self.samples_per_pixel;
                let base_dst = (row * self.chunk_width + col) * self.samples_per_pixel;
                padded[base_dst..base_dst + self.samples_per_pixel]
                    .copy_from_slice(&values[base_src..base_src + self.samples_per_pixel]);
            }
        }

        Ok(Arc::new(ChunkCacheEntry { data: padded }))
    }

    fn fetch_chunk(&self, chunk_index: usize) -> AnyResult<Arc<ChunkCacheEntry>> {
        if let Some(entry) = self.cache.lock().unwrap().get(&chunk_index) {
            return Ok(entry.clone());
        }

        let entry = self.load_chunk(chunk_index)?;
        self.cache
            .lock()
            .unwrap()
            .insert(chunk_index, entry.clone());
        Ok(entry)
    }

    pub fn pixel_scale(&self) -> Option<[f64; 3]> {
        self.pixel_scale
    }

    pub fn tiepoint(&self) -> Option<[f64; 6]> {
        self.tiepoint
    }

    pub fn compute_min_max(&self) -> AnyResult<(f32, f32)> {
        if let Some(result) = *self.min_max.lock().unwrap() {
            return Ok(result);
        }

        let mut min_value = f32::INFINITY;
        let mut max_value = f32::NEG_INFINITY;

        for chunk_index in 0..(self.chunks_across * self.chunks_down) {
            let chunk = self.fetch_chunk(chunk_index)?;
            for &value in &chunk.data {
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

        let result = if min_value.is_infinite() || max_value.is_infinite() {
            (0.0, 0.0)
        } else {
            (min_value, max_value)
        };

        *self.min_max.lock().unwrap() = Some(result);
        Ok(result)
    }
}

impl RasterSource for TiffChunkedRasterSource {
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

        let chunk_col = x / self.chunk_width;
        let chunk_row = y / self.chunk_height;
        let chunk_index = chunk_row * self.chunks_across + chunk_col;

        let within_x = x % self.chunk_width;
        let within_y = y % self.chunk_height;

        let (actual_width, actual_height) = self.chunk_dimensions_for_index(chunk_index);
        if within_x >= actual_width || within_y >= actual_height {
            return None;
        }

        let chunk = match self.fetch_chunk(chunk_index) {
            Ok(entry) => entry,
            Err(_) => return None,
        };

        let offset = (within_y * self.chunk_width + within_x) * self.samples_per_pixel + band;
        chunk.data.get(offset).copied()
    }
}

fn convert_decoding_result(result: DecodingResult) -> Vec<f32> {
    match result {
        DecodingResult::U8(data) => data.into_iter().map(|v| v as f32).collect(),
        DecodingResult::U16(data) => data.into_iter().map(|v| v as f32).collect(),
        DecodingResult::U32(data) => data.into_iter().map(|v| v as f32).collect(),
        DecodingResult::I8(data) => data.into_iter().map(|v| v as f32).collect(),
        DecodingResult::I16(data) => data.into_iter().map(|v| v as f32).collect(),
        DecodingResult::I32(data) => data.into_iter().map(|v| v as f32).collect(),
        DecodingResult::F32(data) => data,
        DecodingResult::F64(data) => data.into_iter().map(|v| v as f32).collect(),
        DecodingResult::F16(data) => data.into_iter().map(|v| f32::from(v)).collect(),
        DecodingResult::U64(data) => data.into_iter().map(|v| v as f32).collect(),
        DecodingResult::I64(data) => data.into_iter().map(|v| v as f32).collect(),
    }
}
