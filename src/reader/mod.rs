pub mod cog;
pub mod compliance;
pub mod local;
pub mod metadata;
pub mod s3_tile_reader;

// Re-export core types from geocog (used internally)
pub use geocog::cog_reader;

#[cfg(test)]
mod gdal_comparison_tests;

#[cfg(all(test, feature = "s3-tests"))]
mod s3_tests;
