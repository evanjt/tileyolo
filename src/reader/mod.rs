pub mod cog;
pub mod cog_reader;
pub mod local;
pub mod lzw_fallback;
pub mod metadata;
pub mod range_reader;
pub mod raster;
pub mod s3;
pub mod tiff_chunked;
pub mod tiff_utils;
pub mod tile_cache;

#[cfg(test)]
pub mod test_utils;

// #[cfg(test)]
// mod tests; // TODO: Update to use cog3pio instead of GDAL
