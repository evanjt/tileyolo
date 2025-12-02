mod config;
mod error;
mod geometry;
mod models;
mod traits;
mod utils;

pub mod endpoints;
pub mod reader;

#[cfg(test)]
mod test_lzw_mod;

// #[cfg(test)]
// mod test_utils; // TODO: Update to use cog3pio instead of GDAL

#[cfg(test)]
mod simple_test;

#[cfg(all(test, feature = "gdal-tests"))]
mod gdal_validation_tests;

// #[cfg(test)]
// mod numerical_tests; // TODO: Update to use cog3pio instead of GDAL

// #[cfg(test)]
// mod xyz_compliance_tests; // TODO: Update to use cog3pio instead of GDAL

// #[cfg(test)]
// mod strict_tests; // TODO: Update to use cog3pio instead of GDAL

pub use config::{Config, Source};
pub use endpoints::router::TileYoloRouter;
pub use endpoints::server::TileServer;
pub use error::TileYoloError;
