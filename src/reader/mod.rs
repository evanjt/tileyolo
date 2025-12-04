pub mod cog;
pub mod compliance;
pub mod local;
pub mod metadata;
pub mod s3_tile_reader;

// Re-export core types from geocog
pub use geocog::cog_reader;
pub use geocog::lzw_fallback;
pub use geocog::range_reader;
pub use geocog::raster;
pub use geocog::s3;
pub use geocog::tiff_chunked;
pub use geocog::tiff_utils;
pub use geocog::tile_cache;

#[cfg(test)]
pub mod test_utils;

#[cfg(test)]
mod gdal_comparison_tests;

#[cfg(all(test, feature = "s3-tests"))]
mod s3_tests;
