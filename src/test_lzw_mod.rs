use crate::reader::raster::RasterSource;
use std::path::PathBuf;

#[test]
fn test_lzw_fallback() {
    let path =
        PathBuf::from("/home/evan/projects/personal/geo/tileyolo/data/grayscale/gray_3857-cog.tif");

    println!("Testing complete LZW fallback for: {}", path.display());

    // Test the LZW fallback directly
    match crate::reader::lzw_fallback::try_read_lzw_tiff_fallback(&path) {
        Ok(source) => {
            println!("✅ SUCCESS: LZW fallback worked!");

            let bands = source.bands();
            let height = source.height();
            let width = source.width();
            println!("Bands: {}, Height: {}, Width: {}", bands, height, width);

            if height > 0 && width > 0 {
                let samples = [
                    source.sample(0, 0, 0),
                    source.sample(0, width / 2, height / 2),
                    source.sample(0, width - 1, height - 1),
                ];
                println!("Sample values: {:?}", samples);

                let mut unique_count = 0;
                for (i, sample) in samples.iter().enumerate() {
                    if let Some(value) = sample {
                        if i == 0
                            || !samples
                                .iter()
                                .take(i)
                                .filter_map(|s| *s)
                                .any(|v| v == *value)
                        {
                            unique_count += 1;
                        }
                    }
                }

                if unique_count == 0 {
                    panic!("No distinct sample values returned by streamed LZW reader");
                }
                println!("Unique sample values: {}", unique_count);
            }
        }
        Err(e) => {
            println!("❌ LZW fallback failed: {}", e);

            if e.to_string().contains("Not LZW compressed") {
                println!("ℹ️  The file is not LZW compressed - this might be the issue");
            }
        }
    }
}
