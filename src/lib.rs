//! # TileYolo
//!
//! A high-performance XYZ tile server for GeoTIFF raster data.
//!
//! TileYolo serves Cloud Optimized GeoTIFF (COG) files as standard XYZ tiles,
//! with automatic style detection, caching, and support for embedding into
//! existing axum applications.
//!
//! ## Features
//!
//! - **Zero Configuration**: Automatic style detection from folder structure
//! - **Pure Rust**: No GDAL dependency for core functionality
//! - **Embeddable**: Use [`TileYoloRouter`] to integrate with existing axum apps
//! - **Standalone**: Use [`TileServer`] for a complete HTTP server
//! - **COG Optimized**: Efficient windowed reading of Cloud Optimized GeoTIFFs
//! - **Built-in Styles**: Viridis, Plasma, Magma, Inferno, and more
//!
//! ## Quick Start (Standalone Server)
//!
//! ```rust,no_run
//! use tileyolo::{Config, Source, TileServer};
//! use std::path::PathBuf;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let config = Config::builder()
//!         .source(Source::Local(PathBuf::from("./data")))
//!         .port(8080)
//!         .build()?;
//!
//!     TileServer::new(config).await?.start().await
//! }
//! ```
//!
//! ## Embedding in Existing Applications
//!
//! ```rust,no_run
//! use axum::Router;
//! use tileyolo::TileYoloRouter;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let tiles = TileYoloRouter::from_directory("./geo_data").await?;
//!
//!     let app = Router::new()
//!         .nest("/tiles", tiles.into_router());
//!
//!     // Add your other routes here...
//!     Ok(())
//! }
//! ```
//!
//! ## Data Organization
//!
//! GeoTIFF files should be organized in style folders:
//!
//! ```text
//! data/
//! ├── viridis/
//! │   ├── elevation.tif
//! │   └── temperature.tif
//! ├── rgb/
//! │   └── satellite.tif
//! └── custom/
//!     ├── layer.tif
//!     └── style.txt  # Custom color stops
//! ```
//!
//! ## HTTP Endpoints
//!
//! - `GET /tiles/{layer}/{z}/{x}/{y}?style=...` - XYZ tile (PNG)
//! - `GET /layers` - JSON list of available layers
//! - `GET /map` - Interactive web viewer

mod config;
mod constants;
mod endpoints;
mod error;
mod models;
mod reader;
mod traits;
mod utils;

// Testing utilities - public for the generate_test_cog binary
pub mod testing;

#[cfg(test)]
mod gdal_validation_tests;

// Public API
pub use config::{Config, ConfigBuilder, Source};
pub use endpoints::router::TileYoloRouter;
pub use endpoints::server::TileServer;
pub use error::TileYoloError;
pub use reader::compliance;
