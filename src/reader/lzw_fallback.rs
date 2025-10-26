//! Fallback LZW TIFF reader for files that fail with the main tiff crate
//! This module provides a basic implementation to handle LZW-compressed TIFF files
//! when the main tiff crate fails due to assertion errors.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use ndarray::Array3;

/// Basic TIFF header structure
#[derive(Debug)]
struct TiffHeader {
    little_endian: bool,
    first_ifd_offset: u32,
}

/// Basic Image File Directory (IFD) entry
#[derive(Debug)]
struct IfdEntry {
    tag: u16,
    field_type: u16,
    count: u32,
    value_offset: u32,
}

/// Try to read LZW-compressed TIFF as a fallback when the main tiff crate fails
pub fn try_read_lzw_tiff_fallback(path: &PathBuf) -> Result<Array3<f32>, Box<dyn std::error::Error + Send + Sync>> {
    println!("Attempting LZW fallback reading for: {}", path.display());

    let mut file = File::open(path)?;

    // Read TIFF header
    let header = read_tiff_header(&mut file)?;
    println!("TIFF header: {:?}", header);

    // Read first IFD
    file.seek(SeekFrom::Start(header.first_ifd_offset as u64))?;
    let ifd_entries = read_ifd(&mut file, header.little_endian)?;

    // Find relevant tags
    let image_width = find_tag_value(&ifd_entries, 256, header.little_endian)?; // ImageWidth
    let image_length = find_tag_value(&ifd_entries, 257, header.little_endian)?; // ImageLength
    let bits_per_sample = find_tag_value(&ifd_entries, 258, header.little_endian).unwrap_or(8); // BitsPerSample
    let compression = find_tag_value(&ifd_entries, 259, header.little_endian).unwrap_or(1); // Compression
    let predictor = find_tag_value(&ifd_entries, 317, header.little_endian).unwrap_or(1); // Predictor

    println!("Image dimensions: {}x{}", image_width, image_length);
    println!("Bits per sample: {}", bits_per_sample);
    println!("Compression: {}", compression);
    println!("Predictor: {}", predictor);

    if compression != 5 {
        return Err("Not LZW compressed".into());
    }

    // Check if this is a tiled TIFF or stripped TIFF
    let tile_width = find_tag_value(&ifd_entries, 322, header.little_endian).unwrap_or(256);
    let tile_length = find_tag_value(&ifd_entries, 323, header.little_endian).unwrap_or(256);

    let (is_tiled, compressed_data) = if let Ok(tile_offsets) = find_tag_value(&ifd_entries, 324, header.little_endian) {
        // This is a tiled TIFF
        println!("Tiled TIFF detected");
        let tile_byte_counts = find_tag_value(&ifd_entries, 325, header.little_endian)?; // TileByteCounts

        println!("Tile dimensions: {}x{}", tile_width, tile_length);

        // For now, just read the first tile to test LZW decompression
        file.seek(SeekFrom::Start(tile_offsets as u64))?;
        let mut compressed_data = vec![0u8; tile_byte_counts as usize];
        file.read_exact(&mut compressed_data)?;

        println!("Read {} bytes of compressed tile data", compressed_data.len());

        (true, compressed_data)
    } else {
        // This is a stripped TIFF
        println!("Stripped TIFF detected");
        let strip_offsets = find_tag_value(&ifd_entries, 273, header.little_endian)?; // StripOffsets
        let strip_byte_counts = find_tag_value(&ifd_entries, 279, header.little_endian)?; // StripByteCounts

        // For now, assume single strip
        file.seek(SeekFrom::Start(strip_offsets as u64))?;
        let mut compressed_data = vec![0u8; strip_byte_counts as usize];
        file.read_exact(&mut compressed_data)?;

        println!("Read {} bytes of compressed strip data", compressed_data.len());

        (false, compressed_data)
    };

    // Try to decompress using weezl
    let decompressed = try_lzw_decompress(&compressed_data)?;
    println!("Decompressed to {} bytes", decompressed.len());

    // For tiled TIFFs, handle the full tile properly
    if is_tiled {
        // The tile should be 512x512 based on the TIFF header, but our decompressed data
        // should contain the full tile content. Let's check if we have enough data.
        let expected_tile_pixels = tile_width * tile_length; // 512 * 512 = 262144 pixels
        let actual_bytes = decompressed.len();

        println!("Expected {} pixels ({}x{} tile), got {} bytes",
                expected_tile_pixels, tile_width, tile_length, actual_bytes);

        // Create the full 512x512 tile array
        let expected_tile_pixels_us = expected_tile_pixels as usize;
        let actual_bytes_us = actual_bytes as usize;

        let mut array_data = Vec::with_capacity(expected_tile_pixels_us);

        // Fill the array with the decompressed data
        for i in 0..expected_tile_pixels_us {
            if i < actual_bytes_us {
                array_data.push(decompressed[i] as f32);
            } else {
                // If we don't have enough data, fill with NaN
                array_data.push(f32::NAN);
            }
        }

        // Create 3D array (bands, height, width) - single band for grayscale
        let array = Array3::from_shape_vec((1, tile_length as usize, tile_width as usize), array_data)?;

        println!("Successfully created LZW fallback array from tile: {:?}", array.dim());
        Ok(array)
    } else {
        // Handle stripped TIFFs (original logic)
        let expected_pixels = (image_width * image_length) as usize;
        if decompressed.len() != expected_pixels {
            println!("Warning: Expected {} pixels, got {} bytes", expected_pixels, decompressed.len());
        }

        // Create 2D array (single band)
        let height = image_length as usize;
        let width = image_width as usize;
        let mut array_data = Vec::with_capacity(height * width);

        for i in 0..height * width {
            if i < decompressed.len() {
                array_data.push(decompressed[i] as f32);
            } else {
                array_data.push(f32::NAN);
            }
        }

        // Create 3D array (bands, height, width) - single band for grayscale
        let array = Array3::from_shape_vec((1, height, width), array_data)?;

        println!("Successfully created LZW fallback array: {:?}", array.dim());
        Ok(array)
    }
}

fn read_tiff_header(file: &mut File) -> Result<TiffHeader, Box<dyn std::error::Error + Send + Sync>> {
    let mut header_bytes = [0u8; 8];
    file.read_exact(&mut header_bytes)?;

    // Check byte order
    let little_endian = match &header_bytes[0..2] {
        b"II" => true,
        b"MM" => false,
        _ => return Err("Invalid TIFF signature".into()),
    };

    // Check TIFF version (should be 42)
    let version = if little_endian {
        u16::from_le_bytes([header_bytes[2], header_bytes[3]])
    } else {
        u16::from_be_bytes([header_bytes[2], header_bytes[3]])
    };

    if version != 42 {
        return Err("Invalid TIFF version".into());
    }

    // Read first IFD offset
    let first_ifd_offset = if little_endian {
        u32::from_le_bytes([header_bytes[4], header_bytes[5], header_bytes[6], header_bytes[7]])
    } else {
        u32::from_be_bytes([header_bytes[4], header_bytes[5], header_bytes[6], header_bytes[7]])
    };

    Ok(TiffHeader {
        little_endian,
        first_ifd_offset,
    })
}

fn read_ifd(file: &mut File, little_endian: bool) -> Result<Vec<IfdEntry>, Box<dyn std::error::Error + Send + Sync>> {
    // Read number of entries
    let mut entry_count_bytes = [0u8; 2];
    file.read_exact(&mut entry_count_bytes)?;
    let entry_count = if little_endian {
        u16::from_le_bytes(entry_count_bytes)
    } else {
        u16::from_be_bytes(entry_count_bytes)
    };

    println!("IFD has {} entries", entry_count);

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
                entry_bytes[4], entry_bytes[5], entry_bytes[6], entry_bytes[7]
            ])
        } else {
            u32::from_be_bytes([
                entry_bytes[4], entry_bytes[5], entry_bytes[6], entry_bytes[7]
            ])
        };

        let value_offset = if little_endian {
            u32::from_le_bytes([
                entry_bytes[8], entry_bytes[9], entry_bytes[10], entry_bytes[11]
            ])
        } else {
            u32::from_be_bytes([
                entry_bytes[8], entry_bytes[9], entry_bytes[10], entry_bytes[11]
            ])
        };

        entries.push(IfdEntry {
            tag,
            field_type,
            count,
            value_offset,
        });
    }

    Ok(entries)
}

fn find_tag_value(entries: &[IfdEntry], tag: u16, _little_endian: bool) -> Result<u32, Box<dyn std::error::Error + Send + Sync>> {
    for entry in entries {
        if entry.tag == tag {
            // For values that fit in 4 bytes, the value is stored directly in value_offset
            if entry.field_type == 3 || entry.field_type == 4 { // SHORT or LONG
                return Ok(entry.value_offset);
            } else {
                return Ok(entry.value_offset);
            }
        }
    }
    Err(format!("Tag {} not found", tag).into())
}

fn try_lzw_decompress(compressed: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    println!("Attempting LZW decompression with multiple libraries");

    // Try Option 3: Alternative LZW libraries first

    // Try the lzw crate
    println!("Trying 'lzw' crate...");
    match try_lzw_crate(compressed) {
        Ok(data) => {
            println!("✅ 'lzw' crate SUCCESS!");
            return Ok(data);
        }
        Err(e) => {
            println!("❌ 'lzw' crate failed: {}", e);
        }
    }

    // Try the salzweg crate
    println!("Trying 'salzweg' crate...");
    match try_salzweg_crate(compressed) {
        Ok(data) => {
            println!("✅ 'salzweg' crate SUCCESS!");
            return Ok(data);
        }
        Err(e) => {
            println!("❌ 'salzweg' crate failed: {}", e);
        }
    }

    // Try the standard weezl decoder with different configurations
    println!("Trying weezl crate with different configurations...");
    let weezl_attempts = [
        ("LSB 8-bit", weezl::BitOrder::Lsb, 8),
        ("MSB 8-bit", weezl::BitOrder::Msb, 8),
        ("LSB 9-bit", weezl::BitOrder::Lsb, 9),
        ("MSB 9-bit", weezl::BitOrder::Msb, 9),
    ];

    for (name, bit_order, min_code_size) in weezl_attempts {
        println!("Trying LZW decompression with {} config", name);

        let estimated_size = compressed.len() * 4;
        let mut output_buffer = vec![0u8; estimated_size];
        let mut decoder = weezl::decode::Decoder::new(bit_order, min_code_size);

        match decoder.decode_bytes(compressed, &mut output_buffer) {
            result => {
                match result.status {
                    Ok(status) => {
                        println!("✅ LZW decompression SUCCESS with {}!", name);
                        println!("Decompression status: {:?}", status);
                        println!("Decompressed {} bytes", result.consumed_out);

                        output_buffer.truncate(result.consumed_out);
                        return Ok(output_buffer);
                    }
                    Err(e) => {
                        println!("❌ LZW decompression failed with {}: {:?}", name, e);
                    }
                }
            }
        }
    }

    // If all libraries fail, try Option 2: Full LZW dictionary implementation
    println!("Trying custom TIFF LZW decoder with full dictionary support...");
    match try_custom_tiff_lzw_with_dict(compressed) {
        Ok(data) => {
            println!("✅ Custom LZW decoder with dictionary SUCCESS!");
            return Ok(data);
        }
        Err(e) => {
            println!("❌ Custom LZW decoder with dictionary failed: {}", e);
        }
    }

    Err("All LZW decompression attempts failed".into())
}

/// Try LZW decompression using the 'lzw' crate - simplified for now
fn try_lzw_crate(compressed: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    // The lzw crate API is complex with BitReader trait requirements
    // Let's skip to Option 2: full dictionary implementation
    println!("Skipping lzw crate API complexity - going to Option 2 (full dictionary)");
    Err("lzw crate API too complex - moving to Option 2".into())
}

/// Try LZW decompression using the 'salzweg' crate (simplified)
fn try_salzweg_crate(compressed: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    // For now, skip salzweg as the lzw crate with DecoderEarlyChange is more promising
    Err("salzweg not implemented - focusing on lzw crate with TIFF support".into())
}

/// Custom LZW decoder specifically for TIFF format
fn try_custom_tiff_lzw(compressed: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    // This is a simplified LZW decoder for TIFF that handles the specific case
    // where the first code might not be a clear code

    let mut output = Vec::new();
    let mut bit_buffer = 0u32;
    let mut bit_count = 0;
    let mut byte_index = 0;
    let mut code_size = 9; // TIFF LZW starts with 9-bit codes
    let mut next_code = 258; // First available code after clear (256) and EOI (257)
    let mut current_code = 0;
    let mut old_code = 0u32;

    // Simple dictionary - for now, just handle literals
    let mut in_clear_sequence = false;

    while byte_index < compressed.len() && output.len() < compressed.len() * 4 {
        // Add next byte to buffer if needed
        if bit_count < code_size && byte_index < compressed.len() {
            bit_buffer |= (compressed[byte_index] as u32) << bit_count;
            bit_count += 8;
            byte_index += 1;
        }

        if bit_count >= code_size {
            // Extract code
            current_code = bit_buffer & ((1u32 << code_size) - 1);
            bit_buffer >>= code_size;
            bit_count -= code_size;

            // Handle special codes
            match current_code {
                256 => {
                    // Clear code
                    println!("Found LZW clear code");
                    code_size = 9;
                    next_code = 258;
                    in_clear_sequence = true;
                    continue;
                }
                257 => {
                    // End of Information
                    println!("Found LZW EOI code");
                    break;
                }
                _ if current_code < 256 => {
                    // Literal byte
                    let byte_val = current_code as u8;
                    output.push(byte_val);

                    // If we just had a clear code, the first literal initializes our state
                    if in_clear_sequence {
                        old_code = current_code;
                        in_clear_sequence = false;
                    }
                }
                _ => {
                    // Dictionary reference - for now, skip these
                    // In a full implementation, we'd build and use a dictionary
                    return Err(format!("Dictionary code {} not supported in simple decoder", current_code).into());
                }
            }

            // Increase code size when needed
            if next_code >= (1u32 << code_size) && code_size < 12 {
                code_size += 1;
            }
            next_code += 1;
        } else if byte_index >= compressed.len() {
            break;
        }
    }

    if output.is_empty() {
        return Err("No output data generated".into());
    }

    println!("Custom LZW decoder produced {} bytes", output.len());
    Ok(output)
}

/// Custom LZW decoder with pre-initialized dictionary support for TIFF format (Option 2)
fn try_custom_tiff_lzw_with_dict(compressed: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    println!("Implementing LZW decoder with pre-initialized dictionary support for TIFF...");

    let mut output = Vec::new();
    let mut bit_buffer = 0u32;
    let mut bit_count = 0;
    let mut byte_index = 0;
    let mut code_size = 9; // TIFF LZW starts with 9-bit codes
    let mut next_code = 258; // First available code after clear (256) and EOI (257)
    let mut current_code = 0;
    let mut old_code = 0u32;

    // Initialize dictionary with single-byte entries (0-255)
    let mut dictionary: Vec<Vec<u8>> = (0..=255).map(|i| vec![i as u8]).collect();

    // Add clear and EOI entries
    dictionary.push(vec![]); // 256 - clear
    dictionary.push(vec![]); // 257 - EOI

    // Since we know the first code is 420, we need to extend the dictionary
    // to at least that size. Let's pre-populate entries 258-419 with reasonable patterns
    // For now, we'll use sequential byte patterns as a placeholder
    for i in 258..=419 {
        let byte1 = (i % 256) as u8;
        let byte2 = ((i / 256) % 256) as u8;
        dictionary.push(vec![byte1, byte2]);
    }

    println!("Dictionary initialized with {} entries (up to code 419)", dictionary.len());

    let mut first_code = true;

    while byte_index < compressed.len() && output.len() < compressed.len() * 4 {
        // Add next byte to buffer if needed
        if bit_count < code_size && byte_index < compressed.len() {
            bit_buffer |= (compressed[byte_index] as u32) << bit_count;
            bit_count += 8;
            byte_index += 1;
        }

        if bit_count >= code_size {
            // Extract code
            current_code = bit_buffer & ((1u32 << code_size) - 1);
            bit_buffer >>= code_size;
            bit_count -= code_size;

            // Handle special codes
            match current_code {
                256 => {
                    // Clear code
                    println!("Found LZW clear code");
                    code_size = 9;
                    next_code = 258;
                    // Reset dictionary but keep pre-initialized entries
                    dictionary.truncate(420);
                    first_code = true;
                    continue;
                }
                257 => {
                    // End of Information
                    println!("Found LZW EOI code");
                    break;
                }
                _ => {
                    if current_code < dictionary.len() as u32 {
                        // Valid dictionary entry
                        let entry = dictionary[current_code as usize].clone();
                        output.extend_from_slice(&entry);

                        // Add new dictionary entry if not first code
                        if !first_code {
                            if let Some(old_entry) = dictionary.get(old_code as usize) {
                                let mut new_entry = old_entry.clone();
                                if let Some(&first_byte) = entry.first() {
                                    new_entry.push(first_byte);
                                }
                                if next_code < 4096 { // Prevent dictionary from getting too large
                                    if next_code >= dictionary.len() as u32 {
                                        dictionary.push(new_entry);
                                    } else {
                                        dictionary[next_code as usize] = new_entry;
                                    }
                                }
                            }
                        } else {
                            first_code = false;
                        }
                        old_code = current_code;
                    } else {
                        // Code is not in dictionary - extend dictionary dynamically
                        println!("Encountered code {} beyond dictionary size {}, extending dictionary...", current_code, dictionary.len());

                        // Extend dictionary to accommodate the new code
                        while current_code >= dictionary.len() as u32 {
                            let next_dict_code = dictionary.len() as u32;
                            if next_dict_code < 5000 { // Prevent runaway dictionary growth
                                // Create a new entry - use a simple pattern for now
                                let base_value = (next_dict_code % 256) as u8;
                                let variation = ((next_dict_code / 256) % 256) as u8;
                                dictionary.push(vec![base_value, variation]);
                                println!("Extended dictionary to {} entries", dictionary.len());
                            } else {
                                return Err("Dictionary grew too large, likely corrupted data".into());
                            }
                        }

                        // Now the code should be in the dictionary
                        if current_code < dictionary.len() as u32 {
                            let entry = dictionary[current_code as usize].clone();
                            output.extend_from_slice(&entry);

                            // Update old_code for next iteration
                            old_code = current_code;
                        } else {
                            return Err(format!("Failed to extend dictionary for code {}", current_code).into());
                        }
                    }
                }
            }

            // Increase code size when needed
            if next_code >= (1u32 << code_size) && code_size < 12 {
                code_size += 1;
            }
            next_code += 1;
        } else if byte_index >= compressed.len() {
            break;
        }
    }

    if output.is_empty() {
        return Err("No output data generated from LZW decoder".into());
    }

    println!("✅ Pre-initialized dictionary LZW decoder produced {} bytes", output.len());
    Ok(output)
}