//! Router builder for embedding `TileYolo` into existing axum applications.
//!
//! This module provides flexible options for integrating `TileYolo` routes
//! into any axum application.
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use axum::Router;
//! use tileyolo::TileYoloRouter;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let tiles = TileYoloRouter::from_directory("./geo_data").await?;
//!
//!     // Option 1: Full router with all endpoints
//!     let app = Router::new().nest("/geo", tiles.into_router());
//!
//!     // Option 2: Individual routes for full control (see examples below)
//!     Ok(())
//! }
//! ```
//!
//! # Individual Route Methods
//!
//! For maximum flexibility, you can add individual endpoints:
//!
//! ```rust,no_run
//! use axum::Router;
//! use tileyolo::TileYoloRouter;
//!
//! # async fn example() -> anyhow::Result<()> {
//! let tiles = TileYoloRouter::from_directory("./data").await?;
//!
//! let app = Router::new()
//!     // Custom path for tiles - use any name you want
//!     .merge(tiles.tile_route("/xyz/{layer}/{z}/{x}/{y}"))
//!     // Optionally add layers endpoint
//!     .merge(tiles.layers_route("/api/layers"))
//!     // Optionally add map viewer
//!     .merge(tiles.map_route("/viewer"));
//! # Ok(())
//! # }
//! ```

use crate::config::Source;
use crate::endpoints::handlers::{get_all_layers, tile_handler, webmap_handler};
use crate::error::TileYoloError;
use crate::reader::local::LocalTileReader;
use crate::traits::TileReader;
use axum::{routing::get, Router};
use std::path::PathBuf;
use std::sync::Arc;

/// Builder for creating `TileYolo` routes that can be embedded in existing axum applications.
///
/// Provides both convenience methods for common setups and granular methods
/// for full control over route paths.
///
/// # Examples
///
/// ## Full Router (All Endpoints)
/// ```rust,no_run
/// # use tileyolo::TileYoloRouter;
/// # async fn example() -> anyhow::Result<()> {
/// let tiles = TileYoloRouter::from_directory("./data").await?;
/// let router = tiles.into_router(); // /tiles, /layers, /map
/// # Ok(())
/// # }
/// ```
///
/// ## Custom Tile Path
/// ```rust,no_run
/// # use axum::Router;
/// # use tileyolo::TileYoloRouter;
/// # async fn example() -> anyhow::Result<()> {
/// let tiles = TileYoloRouter::from_directory("./data").await?;
///
/// // Just tiles at a custom path
/// let app = Router::new()
///     .merge(tiles.tile_route("/{layer}/{z}/{x}/{y}"));
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct TileYoloRouter {
    reader: Arc<dyn TileReader>,
}

impl TileYoloRouter {
    /// Create a `TileYolo` router from a local directory path.
    ///
    /// The directory should contain `GeoTIFF` files organized by style folders.
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

    /// Create a `TileYolo` router from a configured source.
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
    ///
    /// # Errors
    ///
    /// Returns `TileYoloError::Unsupported` if attempting to use an unimplemented backend.
    pub async fn from_source(source: Source) -> Result<Self, TileYoloError> {
        let reader: Arc<dyn TileReader> = match source {
            Source::Local(path) => Arc::new(LocalTileReader::new(&path).await),
            Source::S3 { bucket, prefix } => {
                return Err(TileYoloError::Unsupported(format!(
                    "S3 backend not yet implemented (bucket: {bucket}, prefix: {prefix})"
                )));
            }
        };
        Ok(Self { reader })
    }

    /// Create a `TileYolo` router from a custom `TileReader` implementation.
    ///
    /// This allows you to provide your own data source implementation.
    pub fn from_reader(reader: Arc<dyn TileReader>) -> Self {
        Self { reader }
    }

    /// Get a reference to the underlying `TileReader`.
    ///
    /// Useful for querying layer information before starting the server.
    #[must_use] pub fn reader(&self) -> &Arc<dyn TileReader> {
        &self.reader
    }

    /// List all available layers.
    ///
    /// Returns layer metadata including names, styles, and geometry information.
    pub async fn list_layers(&self) -> Vec<crate::models::layer::Layer> {
        self.reader.list_layers().await
    }

    // ========================================================================
    // Individual Route Methods - For granular control
    // ========================================================================

    /// Create a router with just the XYZ tile endpoint at a custom path.
    ///
    /// The path must include `{layer}`, `{z}`, `{x}`, and `{y}` parameters.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use axum::Router;
    /// # use tileyolo::TileYoloRouter;
    /// # async fn example() -> anyhow::Result<()> {
    /// let tiles = TileYoloRouter::from_directory("./data").await?;
    ///
    /// // Standard XYZ path
    /// let app = Router::new()
    ///     .merge(tiles.tile_route("/{layer}/{z}/{x}/{y}"));
    ///
    /// // Or with a prefix
    /// let app = Router::new()
    ///     .merge(tiles.tile_route("/xyz/{layer}/{z}/{x}/{y}"));
    /// # Ok(())
    /// # }
    /// ```
    pub fn tile_route(&self, path: &str) -> Router {
        Router::new()
            .route(path, get(tile_handler))
            .with_state(self.reader.clone())
    }

    /// Create a router with just the layers JSON endpoint at a custom path.
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
    ///     .merge(tiles.layers_route("/api/geo/layers"));
    /// # Ok(())
    /// # }
    /// ```
    pub fn layers_route(&self, path: &str) -> Router {
        Router::new()
            .route(path, get(get_all_layers))
            .with_state(self.reader.clone())
    }

    /// Create a router with just the interactive map viewer at a custom path.
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
    ///     .merge(tiles.map_route("/viewer"));
    /// # Ok(())
    /// # }
    /// ```
    pub fn map_route(&self, path: &str) -> Router {
        Router::new()
            .route(path, get(webmap_handler))
            .with_state(self.reader.clone())
    }

    // ========================================================================
    // Convenience Methods - For common setups
    // ========================================================================

    /// Convert into a full axum Router with all endpoints.
    ///
    /// # Routes
    ///
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
    ///     .nest("/geo", tiles.into_router());
    /// // Results in: /geo/tiles/{layer}/{z}/{x}/{y}, /geo/layers, /geo/map
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

    /// Convert into an axum Router with tiles and layers (no map viewer).
    ///
    /// Use this if you only need the API endpoints without the interactive viewer.
    ///
    /// # Routes
    ///
    /// - `GET /tiles/{layer}/{z}/{x}/{y}?style=...` - XYZ tile endpoint
    /// - `GET /layers` - JSON layer listing
    pub fn into_api_router(self) -> Router {
        Router::new()
            .route("/tiles/{layer}/{z}/{x}/{y}", get(tile_handler))
            .route("/layers", get(get_all_layers))
            .with_state(self.reader)
    }

    /// Convert into an axum Router with only the tile endpoint.
    ///
    /// # Routes
    ///
    /// - `GET /tiles/{layer}/{z}/{x}/{y}?style=...` - XYZ tile endpoint
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
    ///     .nest("/xyz", tiles.into_tiles_only_router());
    /// // Results in: /xyz/tiles/{layer}/{z}/{x}/{y}
    /// # Ok(())
    /// # }
    /// ```
    pub fn into_tiles_only_router(self) -> Router {
        Router::new()
            .route("/tiles/{layer}/{z}/{x}/{y}", get(tile_handler))
            .with_state(self.reader)
    }
}
