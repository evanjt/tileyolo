use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub type AnyResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub const TAG_IMAGE_WIDTH: u16 = 256;
pub const TAG_IMAGE_LENGTH: u16 = 257;
pub const TAG_BITS_PER_SAMPLE: u16 = 258;
pub const TAG_COMPRESSION: u16 = 259;
pub const TAG_STRIP_OFFSETS: u16 = 273;
pub const TAG_SAMPLES_PER_PIXEL: u16 = 277;
pub const TAG_ROWS_PER_STRIP: u16 = 278;
pub const TAG_STRIP_BYTE_COUNTS: u16 = 279;
pub const TAG_PLANAR_CONFIGURATION: u16 = 284;
pub const TAG_PREDICTOR: u16 = 317;
pub const TAG_TILE_WIDTH: u16 = 322;
pub const TAG_TILE_LENGTH: u16 = 323;
pub const TAG_TILE_OFFSETS: u16 = 324;
pub const TAG_TILE_BYTE_COUNTS: u16 = 325;
pub const TAG_SAMPLE_FORMAT: u16 = 339;
pub const TAG_MODEL_PIXEL_SCALE: u16 = 33550;
pub const TAG_MODEL_TIEPOINT: u16 = 33922;
pub const TAG_GDAL_METADATA: u16 = 42112;
pub const TAG_GDAL_NODATA: u16 = 42113;

#[derive(Debug)]
pub struct TiffHeader {
    pub little_endian: bool,
    pub first_ifd_offset: u32,
}

#[derive(Debug, Clone)]
pub struct IfdEntry {
    pub tag: u16,
    pub field_type: u16,
    pub count: u32,
    pub value_offset: u32,
    pub raw_value_bytes: [u8; 4],
}

pub fn read_tiff_header(file: &mut File) -> AnyResult<TiffHeader> {
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

pub fn read_ifd(file: &mut File, little_endian: bool) -> AnyResult<Vec<IfdEntry>> {
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

pub fn read_entry_values_u32(
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
        file.seek(SeekFrom::Start(u64::from(entry.value_offset)))?;
        file.read_exact(&mut raw_bytes)?;
        file.seek(SeekFrom::Start(current_pos))?;
    }

    let mut values = Vec::with_capacity(entry.count as usize);
    for chunk in raw_bytes.chunks_exact(type_size) {
        let value = match entry.field_type {
            3 => {
                if little_endian {
                    u32::from(u16::from_le_bytes([chunk[0], chunk[1]]))
                } else {
                    u32::from(u16::from_be_bytes([chunk[0], chunk[1]]))
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

pub fn read_entry_values_f64(
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
    file.seek(SeekFrom::Start(u64::from(entry.value_offset)))?;
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

pub fn read_tag_u32(
    file: &mut File,
    entries: &[IfdEntry],
    tag: u16,
    little_endian: bool,
) -> AnyResult<u32> {
    let entry = get_entry(entries, tag).ok_or_else(|| format!("Tag {tag} not found"))?;
    let values = read_entry_values_u32(file, entry, little_endian)?;
    values.first()
        .copied()
        .ok_or_else(|| format!("Tag {tag} missing value").into())
}

pub fn read_tag_u32_vec(
    file: &mut File,
    entries: &[IfdEntry],
    tag: u16,
    little_endian: bool,
) -> AnyResult<Vec<u32>> {
    let entry = get_entry(entries, tag).ok_or_else(|| format!("Tag {tag} not found"))?;
    read_entry_values_u32(file, entry, little_endian)
}

pub fn read_tag_u32_vec_optional(
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

pub fn read_tag_string_from_ifd(
    file: &mut File,
    entries: &[IfdEntry],
    little_endian: bool,
    tag: u16,
) -> AnyResult<String> {
    let entry = get_entry(entries, tag).ok_or_else(|| format!("Tag {tag} not found"))?;
    let total_bytes = entry.count as usize;
    let mut raw_bytes = vec![0u8; total_bytes];

    let inline = matches!(entry.field_type, 1 | 2 | 7) && total_bytes <= 4;

    if inline {
        if little_endian {
            raw_bytes.copy_from_slice(&entry.raw_value_bytes[..total_bytes]);
        } else {
            let start = 4 - total_bytes;
            raw_bytes.copy_from_slice(&entry.raw_value_bytes[start..]);
        }
    } else {
        let current_pos = file.stream_position()?;
        file.seek(SeekFrom::Start(u64::from(entry.value_offset)))?;
        file.read_exact(&mut raw_bytes)?;
        file.seek(SeekFrom::Start(current_pos))?;
    }

    if let Some(pos) = raw_bytes.iter().position(|&b| b == 0) {
        raw_bytes.truncate(pos);
    }

    Ok(String::from_utf8_lossy(&raw_bytes).to_string())
}

fn get_entry(entries: &[IfdEntry], tag: u16) -> Option<&IfdEntry> {
    entries.iter().find(|entry| entry.tag == tag)
}

#[must_use] pub fn parse_gdal_metadata_stats(metadata: &str) -> Option<(f32, f32)> {
    let min = extract_metadata_value(metadata, "STATISTICS_MINIMUM")?;
    let max = extract_metadata_value(metadata, "STATISTICS_MAXIMUM")?;
    Some((min, max))
}

pub fn read_primary_compression(path: &Path) -> AnyResult<Option<u16>> {
    let mut file = File::open(path)?;
    let header = read_tiff_header(&mut file)?;
    file.seek(SeekFrom::Start(u64::from(header.first_ifd_offset)))?;
    let ifd_entries = read_ifd(&mut file, header.little_endian)?;
    match read_tag_u32(
        &mut file,
        &ifd_entries,
        TAG_COMPRESSION,
        header.little_endian,
    ) {
        Ok(value) => Ok(Some(value as u16)),
        Err(_) => Ok(None),
    }
}

pub fn read_tag_f64_triplet(
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

pub fn read_tag_f64_six(
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

fn extract_metadata_value(metadata: &str, key: &str) -> Option<f32> {
    let needle = format!("name=\"{key}\"");
    let pos = metadata.find(&needle)?;
    let rest = &metadata[pos..];
    let start = rest.find('>')? + 1;
    let rest = &rest[start..];
    let end = rest.find('<')?;
    rest[..end].trim().parse::<f32>().ok()
}
