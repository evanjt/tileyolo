use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

fn main() {
    let path = PathBuf::from("/home/evan/projects/personal/geo/tileyolo/data/grayscale/gray_3857-cog.tif");
    let mut file = File::open(&path).unwrap();

    // Read TIFF header
    let mut header_bytes = [0u8; 8];
    file.read_exact(&mut header_bytes).unwrap();
    let little_endian = match &header_bytes[0..2] {
        b"II" => true,
        b"MM" => false,
        _ => panic!("Invalid TIFF signature"),
    };

    let first_ifd_offset = if little_endian {
        u32::from_le_bytes([header_bytes[4], header_bytes[5], header_bytes[6], header_bytes[7]])
    } else {
        u32::from_be_bytes([header_bytes[4], header_bytes[5], header_bytes[6], header_bytes[7]])
    };

    println!("TIFF header: little_endian={}, first_ifd_offset={}", little_endian, first_ifd_offset);

    // Read IFD
    file.seek(SeekFrom::Start(first_ifd_offset as u64)).unwrap();
    let mut entry_count_bytes = [0u8; 2];
    file.read_exact(&mut entry_count_bytes).unwrap();
    let entry_count = if little_endian {
        u16::from_le_bytes(entry_count_bytes)
    } else {
        u16::from_be_bytes(entry_count_bytes)
    };

    println!("IFD has {} entries", entry_count);

    // Read IFD entries and find tile offsets
    let mut tile_offsets = 0;
    let mut tile_byte_counts = 0;
    let mut compression = 1;

    for _ in 0..entry_count {
        let mut entry_bytes = [0u8; 12];
        file.read_exact(&mut entry_bytes).unwrap();

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
            u32::from_le_bytes([entry_bytes[4], entry_bytes[5], entry_bytes[6], entry_bytes[7]])
        } else {
            u32::from_be_bytes([entry_bytes[4], entry_bytes[5], entry_bytes[6], entry_bytes[7]])
        };

        let value_offset = if little_endian {
            u32::from_le_bytes([entry_bytes[8], entry_bytes[9], entry_bytes[10], entry_bytes[11]])
        } else {
            u32::from_be_bytes([entry_bytes[8], entry_bytes[9], entry_bytes[10], entry_bytes[11]])
        };

        match tag {
            259 => compression = value_offset, // Compression
            324 => tile_offsets = value_offset, // TileOffsets
            325 => tile_byte_counts = value_offset, // TileByteCounts
            _ => {}
        }

        if tag == 259 || tag == 324 || tag == 325 {
            println!("Tag {}: type={}, count={}, value={}", tag, field_type, count, value_offset);
        }
    }

    println!("Compression: {}", compression);
    println!("Tile offsets: {}", tile_offsets);
    println!("Tile byte counts: {}", tile_byte_counts);

    if compression == 5 && tile_offsets > 0 && tile_byte_counts > 0 {
        // Read first tile data
        file.seek(SeekFrom::Start(tile_offsets as u64)).unwrap();
        let mut compressed_data = vec![0u8; tile_byte_counts as usize];
        file.read_exact(&mut compressed_data).unwrap();

        println!("Read {} bytes of compressed data", compressed_data.len());

        // Analyze LZW stream
        analyze_lzw_stream(&compressed_data);
    }
}

fn analyze_lzw_stream(data: &[u8]) {
    println!("Analyzing LZW stream of {} bytes", data.len());

    // Print first few bytes in hex
    println!("First 32 bytes:");
    for (i, chunk) in data.chunks(16).enumerate() {
        print!("{:04x}: ", i * 16);
        for &byte in chunk {
            print!("{:02x} ", byte);
        }
        println!();
        if i >= 1 { break; } // Only first 32 bytes
    }

    // Look for LZW clear code and end-of-information code patterns
    // In standard LZW, clear code is typically 256 (for 8-bit data)
    // and EOI code is 257

    println!("\nLooking for LZW patterns...");

    // Try to decode bit by bit to see if we can identify the issue
    analyze_bit_patterns(data);
}

fn analyze_bit_patterns(data: &[u8]) {
    println!("Bit pattern analysis:");

    // LZW codes are variable length, starting at 9 bits and increasing
    // Let's try to manually decode the first few codes to see the pattern

    let mut bit_buffer = 0u32;
    let mut bit_count = 0;
    let mut byte_index = 0;
    let mut codes_decoded = 0;

    while byte_index < data.len().min(8) && codes_decoded < 10 {
        // Add next byte to buffer
        if byte_index < data.len() {
            bit_buffer |= (data[byte_index] as u32) << bit_count;
            bit_count += 8;
            byte_index += 1;
        }

        // Try to extract codes of different lengths
        for code_size in 9..=12 {
            if bit_count >= code_size {
                let code = bit_buffer & ((1u32 << code_size) - 1);
                println!("  Code size {}: 0x{:x}", code_size, code);

                // Check for special codes
                match code {
                    256 => println!("    -> LZW Clear code"),
                    257 => println!("    -> LZW End of Information code"),
                    _ if code < 256 => println!("    -> Literal byte: 0x{:02x} ('{}')", code, code as u8 as char),
                    _ => println!("    -> Dictionary reference"),
                }

                codes_decoded += 1;
                break;
            }
        }

        // Remove consumed bits
        if bit_count >= 9 {
            bit_buffer >>= 9;
            bit_count -= 9;
        }
    }

    if codes_decoded == 0 {
        println!("Could not decode any LZW codes - stream might be corrupted or use different format");
    }
}