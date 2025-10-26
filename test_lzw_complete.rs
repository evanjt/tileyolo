// Complete test of LZW fallback mechanism
use std::path::PathBuf;

fn main() {
    let path = PathBuf::from("/home/evan/projects/personal/geo/tileyolo/data/grayscale/gray_3857-cog.tif");

    println!("Testing complete LZW fallback for: {}", path.display());

    // Test the LZW fallback directly
    match tileyolo::reader::lzw_fallback::try_read_lzw_tiff_fallback(&path) {
        Ok(array) => {
            println!("✅ SUCCESS: LZW fallback worked!");
            println!("Array dimensions: {:?}", array.dim());

            // Check some sample values
            let (bands, height, width) = array.dim();
            println!("Bands: {}, Height: {}, Width: {}", bands, height, width);

            if height > 0 && width > 0 {
                // Sample a few values from different parts of the image
                let samples = [
                    array[[0, 0, 0]],           // Top-left
                    array[[0, height/2, width/2]], // Center
                    array[[0, height-1, width-1]], // Bottom-right
                ];
                println!("Sample values: {:?}", samples);

                // Check if values are reasonable (not all NaN or the same)
                let unique_values: std::collections::HashSet<_> = samples.iter().collect();
                println!("Unique sample values: {}", unique_values.len());
            }
        }
        Err(e) => {
            println!("❌ LZW fallback failed: {}", e);

            // Try to get more detailed error information
            if e.to_string().contains("Not LZW compressed") {
                println!("ℹ️  The file is not LZW compressed - this might be the issue");
            }
        }
    }
}