//! Router builder for embedding TileYolo into existing axum applications.
//!
//! This module provides a builder pattern for creating a TileYolo router
//! that can be nested into any axum application at a path of your choice.
//!
//! # Example
//!
//! ```rust,no_run
//! use axum::Router;
//! use tileyolo::TileYoloRouter;
//! use std::path::PathBuf;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     // Build the TileYolo router
//!     let tiles = TileYoloRouter::from_directory("./geo_data")
//!         .await?
//!         .into_router();
//!
//!     // Nest it into your application at any path
//!     let app = Router::new()
//!         .nest("/geo", tiles)
//!         // ... your other routes
//!         .route("/health", axum::routing::get(|| async { "ok" }));
//!
//!     // Serve with your own server setup
//!     let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
//!     axum::serve(listener, app).await?;
//!     Ok(())
//! }
//! ```
//!
//! # Routes Provided
//!
//! The router provides these endpoints (relative to where you nest it):
//!
//! - `GET /tiles/{layer}/{z}/{x}/{y}` - XYZ tile endpoint (PNG)
//! - `GET /layers` - JSON list of available layers
//! - `GET /map` - Interactive web map viewer

use crate::config::Source;
use crate::endpoints::handlers::{get_all_layers, tile_handler, webmap_handler};
use crate::reader::local::LocalTileReader;
use crate::traits::TileReader;
use axum::{routing::get, Router};
use std::path::PathBuf;
use std::sync::Arc;

/// Builder for creating a TileYolo router that can be embedded in existing axum applications.
///
/// Use [`TileYoloRouter::from_directory`] to create a router from a local directory,
/// or [`TileYoloRouter::from_source`] for more advanced configuration.
pub struct TileYoloRouter {
    reader: Arc<dyn TileReader>,
}

impl TileYoloRouter {
    /// Create a TileYolo router from a local directory path.
    ///
    /// The directory should contain GeoTIFF files organized by style folders.
    /// This method performs async initialization (scanning files, loading metadata).
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use tileyolo::TileYoloRouter;
    /// # async fn example() -> anyhow::Result<()> {
    /// let tiles = TileYoloRouter::from_directory("./data/geotiffs").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn from_directory<P: Into<PathBuf>>(path: P) -> anyhow::Result<Self> {
        let path = path.into();
        let abs_path = if path.is_absolute() {
            path
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path)
        };

        let reader: Arc<dyn TileReader> = Arc::new(LocalTileReader::new(&abs_path).await);
        Ok(Self { reader })
    }

    /// Create a TileYolo router from a configured source.
    ///
    /// This allows for more advanced configuration including S3 backends (when implemented).
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use tileyolo::{TileYoloRouter, Source};
    /// # use std::path::PathBuf;
    /// # async fn example() -> anyhow::Result<()> {
    /// let source = Source::Local(PathBuf::from("./data"));
    /// let tiles = TileYoloRouter::from_source(source).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn from_source(source: Source) -> anyhow::Result<Self> {
        let reader: Arc<dyn TileReader> = match source {
            Source::Local(path) => Arc::new(LocalTileReader::new(&path).await),
            Source::S3 { .. } => anyhow::bail!("S3 backend is not yet implemented"),
        };
        Ok(Self { reader })
    }

    /// Create a TileYolo router from a custom TileReader implementation.
    ///
    /// This allows you to provide your own data source implementation.
    pub fn from_reader(reader: Arc<dyn TileReader>) -> Self {
        Self { reader }
    }

    /// Get a reference to the underlying TileReader.
    ///
    /// Useful for querying layer information before starting the server.
    pub fn reader(&self) -> &Arc<dyn TileReader> {
        &self.reader
    }

    /// List all available layers.
    ///
    /// Returns layer metadata including names, styles, and geometry information.
    pub async fn list_layers(&self) -> Vec<crate::models::layer::Layer> {
        self.reader.list_layers().await
    }

    /// Convert into an axum Router that can be nested into your application.
    ///
    /// # Routes
    ///
    /// The returned router includes:
    /// - `GET /tiles/{layer}/{z}/{x}/{y}?style=...` - XYZ tile endpoint
    /// - `GET /layers` - JSON layer listing
    /// - `GET /map` - Interactive web viewer
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use axum::Router;
    /// # use tileyolo::TileYoloRouter;
    /// # async fn example() -> anyhow::Result<()> {
    /// let tiles = TileYoloRouter::from_directory("./data").await?;
    ///
    /// let app = Router::new()
    ///     .nest("/api/tiles", tiles.into_router());
    /// # Ok(())
    /// # }
    /// ```
    pub fn into_router(self) -> Router {
        Router::new()
            .route("/tiles/{layer}/{z}/{x}/{y}", get(tile_handler))
            .route("/layers", get(get_all_layers))
            .route("/map", get(webmap_handler))
            .with_state(self.reader)
    }

    /// Convert into an axum Router without the /map viewer endpoint.
    ///
    /// Use this if you only need the API endpoints without the interactive viewer.
    pub fn into_api_router(self) -> Router {
        Router::new()
            .route("/tiles/{layer}/{z}/{x}/{y}", get(tile_handler))
            .route("/layers", get(get_all_layers))
            .with_state(self.reader)
    }
}
