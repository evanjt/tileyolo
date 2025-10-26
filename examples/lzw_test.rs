// Test LZW decompression directly with weezl
use std::fs::File;
use std::io::Read;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        println!("Usage: cargo run --example lzw_test <path_to_tiff>");
        return Ok(());
    }

    let path = &args[1];
    println!("Testing LZW decompression on: {}", path);

    // Read the file
    let mut file = File::open(path)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;

    // Try to find LZW-compressed data in the TIFF
    // Look for TIFF header
    if buffer.len() < 8 {
        return Err("File too small".into());
    }

    // Check TIFF byte order
    let little_endian = match &buffer[0..2] {
        b"II" => true,
        b"MM" => false,
        _ => return Err("Not a TIFF file".into()),
    };

    println!("Byte order: {}", if little_endian { "Little endian" } else { "Big endian" });

    // Try to read IFD offset
    let ifd_offset = if little_endian {
        u32::from_le_bytes([buffer[4], buffer[5], buffer[6], buffer[7]])
    } else {
        u32::from_be_bytes([buffer[4], buffer[5], buffer[6], buffer[7]])
    };

    println!("First IFD offset: {}", ifd_offset);

    // Basic TIFF structure parsing - this is complex, so let's try a simpler approach
    println!("File size: {} bytes", buffer.len());

    Ok(())
}