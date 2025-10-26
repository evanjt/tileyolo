use std::path::PathBuf;

fn main() {
    let path = PathBuf::from("/home/evan/projects/personal/geo/tileyolo/data/grayscale/gray_3857-cog.tif");

    println!("Testing LZW fallback with: {:?}", path);

    match tileyolo::reader::lzw_fallback::try_read_lzw_tiff_fallback(&path) {
        Ok(array) => {
            println!("SUCCESS: LZW fallback worked!");
            println!("Array dimensions: {:?}", array.dim());
            println!("Sample values: {:?}", &array.slice(ndarray::s![0, 0..10, 0..10]));
        }
        Err(e) => {
            println!("LZW fallback failed: {}", e);
        }
    }
}