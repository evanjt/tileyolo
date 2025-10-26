// Simple test to check LZW fallback
use std::path::PathBuf;
use std::fs::File;
use std::io::Read;

fn try_basic_read() -> Result<(), Box<dyn std::error::Error>> {
    let path = PathBuf::from("/home/evan/projects/personal/geo/tileyolo/data/grayscale/gray_3857-cog.tif");
    let mut file = File::open(&path)?;

    println!("Testing basic TIFF file access...");
    let mut header_bytes = [0u8; 8];
    file.read_exact(&mut header_bytes)?;

    println!("First 8 bytes: {:?}", header_bytes);

    // Check byte order
    let byte_order = match &header_bytes[0..2] {
        b"II" => "Little-endian",
        b"MM" => "Big-endian",
        _ => return Err("Invalid TIFF signature".into()),
    };

    // Check TIFF version (should be 42)
    let version = if byte_order == "Little-endian" {
        u16::from_le_bytes([header_bytes[2], header_bytes[3]])
    } else {
        u16::from_be_bytes([header_bytes[2], header_bytes[3]])
    };

    println!("Byte order: {}", byte_order);
    println!("TIFF version: {}", version);

    if version != 42 {
        return Err("Invalid TIFF version".into());
    }

    // Read first IFD offset
    let first_ifd_offset = if byte_order == "Little-endian" {
        u32::from_le_bytes([header_bytes[4], header_bytes[5], header_bytes[6], header_bytes[7]])
    } else {
        u32::from_be_bytes([header_bytes[4], header_bytes[5], header_bytes[6], header_bytes[7]])
    };

    println!("First IFD offset: {}", first_ifd_offset);
    println!("Successfully read TIFF header from: {}", path.display());
    Ok(())
}

fn main() {
    match try_basic_read() {
        Ok(()) => println!("Basic TIFF reading works!"),
        Err(e) => println!("Basic TIFF reading failed: {}", e),
    }
}