use crate::config::{Config, Source};
use crate::endpoints::handlers::{get_all_layers, tile_handler, webmap_handler};
use crate::error::TileYoloError;
use crate::reader::local::LocalTileReader;
use crate::traits::TileReader;
use axum::{Router, routing::get};
use std::net::SocketAddr;
use std::sync::Arc;

pub fn create_router(reader: Arc<dyn TileReader>) -> Router {
    Router::new()
        .route("/tiles/{layer}/{z}/{x}/{y}", get(tile_handler))
        .route("/layers", get(get_all_layers))
        .route("/map", get(webmap_handler))
        .with_state(reader)
}

pub struct TileServer {
    config: Config,
    reader: Arc<dyn TileReader>,
}

impl TileServer {
    pub async fn new(config: Config) -> Result<Self, TileYoloError> {
        let reader: Arc<dyn TileReader> = match &config.source {
            Some(Source::Local(path)) => Arc::new(LocalTileReader::new(path).await),
            Some(Source::S3 { bucket, prefix }) => {
                return Err(TileYoloError::Unsupported(format!(
                    "S3 backend not yet implemented (bucket: {bucket}, prefix: {prefix})"
                )));
            }
            None => {
                return Err(TileYoloError::Config(
                    "No source provided in the configuration".to_string(),
                ));
            }
        };

        Ok(Self { config, reader })
    }

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
