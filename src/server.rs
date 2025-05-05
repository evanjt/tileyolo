use crate::config::{Config, Source};
use crate::reader::TileReader;
use crate::reader::local::LocalTileReader;
use crate::routes::{get_all_layers, tile_handler, webmap_handler};
use axum::{Router, routing::get};
use std::net::SocketAddr;
use std::sync::Arc;

pub struct TileServer {
    config: Config,
    reader: Arc<dyn TileReader>,
}

impl TileServer {
    pub fn new(config: Config) -> anyhow::Result<Self> {
        let reader: Arc<dyn TileReader> = match &config.source {
            Some(Source::Local(path)) => Arc::new(LocalTileReader::new(path)),
            Some(Source::S3 { .. }) => unimplemented!("S3 backend is not yet implemented"),
            None => anyhow::bail!("No source provided in the configuration"),
        };

        // if reader
        Ok(Self { config, reader })
    }

    pub async fn start(self) -> anyhow::Result<()> {
        // Tile-serving router with state
        let app = Router::new()
            .route("/tiles/{layer}/{z}/{x}/{y}", get(tile_handler))
            .route("/layers", get(get_all_layers))
            .route("/map", get(webmap_handler))
            .with_state(self.reader.clone());

        let addr = SocketAddr::from(([0, 0, 0, 0], self.config.port));
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();

        // Choose a random layer for the example URL
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

        let random_layer = layers.first().unwrap().layer.clone();

        println!(
            r#"
    🚀 TileYolo serving on {}

    🗺️ QGIS XYZ-tiles path (on randomly picked layer: {})
       → http://{}/tiles/{}/{{z}}/{{x}}/{{y}}

    🌍 Browse all loaded layers visually
       → http://{}/map

    📚 Query for all layers (JSON)
       → http://{}/layers
            "#,
            addr, random_layer, addr, random_layer, addr, addr
        );

        axum::serve(listener, app.into_make_service())
            .await
            .unwrap();

        Ok(())
    }
}
