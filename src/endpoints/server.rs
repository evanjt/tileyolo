//! Standalone tile server implementation.

use crate::config::{Config, Source};
use crate::endpoints::handlers::{get_all_layers, tile_handler, webmap_handler};
use crate::error::TileYoloError;
use crate::reader::local::LocalTileReader;
use crate::reader::s3_tile_reader::S3TileReader;
use crate::traits::TileReader;
use axum::{routing::get, Router};
use std::net::SocketAddr;
use std::sync::Arc;

/// Create an axum Router with all tile server endpoints.
pub fn create_router(reader: Arc<dyn TileReader>) -> Router {
    Router::new()
        .route("/tiles/{layer}/{z}/{x}/{y}", get(tile_handler))
        .route("/layers", get(get_all_layers))
        .route("/map", get(webmap_handler))
        .with_state(reader)
}

/// Standalone XYZ tile server.
///
/// Use this for running TileYolo as a complete HTTP server.
/// For embedding into existing applications, see [`TileYoloRouter`](crate::TileYoloRouter).
///
/// # Example
///
/// ```rust,no_run
/// use tileyolo::{Config, Source, TileServer};
/// use std::path::PathBuf;
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     let config = Config::builder()
///         .source(Source::Local(PathBuf::from("./data")))
///         .port(8080)
///         .build()?;
///
///     TileServer::new(config).await?.start().await
/// }
/// ```
pub struct TileServer {
    config: Config,
    reader: Arc<dyn TileReader>,
}

impl TileServer {
    /// Create a new tile server from the given configuration.
    ///
    /// This initializes the data source and loads layer metadata.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - No source is configured
    /// - The source type is not implemented (e.g., S3)
    /// - The data source cannot be accessed
    pub async fn new(config: Config) -> Result<Self, TileYoloError> {
        let reader: Arc<dyn TileReader> = match &config.source {
            Some(Source::Local(path)) => Arc::new(LocalTileReader::new(path).await),
            Some(Source::S3 { bucket, prefix }) => Arc::new(S3TileReader::new(bucket, prefix).await?),
            None => {
                return Err(TileYoloError::Config(
                    "No source provided in the configuration".to_string(),
                ));
            }
        };

        Ok(Self { config, reader })
    }

    /// Start the HTTP server and begin serving tiles.
    ///
    /// This method blocks until the server is shut down.
    ///
    /// # Errors
    ///
    /// Returns an error if the server cannot bind to the configured port.
    pub async fn start(self) -> anyhow::Result<()> {
        let app = create_router(self.reader.clone());

        let addr = SocketAddr::from(([0, 0, 0, 0], self.config.port));
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to bind to {addr}: {e}"))?;

        let layers = self.reader.list_layers().await;

        if layers.is_empty() {
            println!(
                "⚠️ No layers found in the data folder.\n\n\
                Define the root data path with the --data-folder flag and be \
                sure to nest the TIFFs in folders according to style. See \
                README for details."
            );
            return Ok(());
        }

        let example_layer = &layers[0].layer;

        println!(
            r"
    🚀 TileYolo serving on {addr}

    🗺️ QGIS XYZ-tiles path (example layer: {example_layer})
       → http://{addr}/tiles/{example_layer}/{{z}}/{{x}}/{{y}}

    🌍 Browse all loaded layers visually
       → http://{addr}/map

    📚 Query for all layers (JSON)
       → http://{addr}/layers
            "
        );

        axum::serve(listener, app.into_make_service())
            .await
            .map_err(|e| anyhow::anyhow!("Server error: {e}"))?;

        Ok(())
    }
}
