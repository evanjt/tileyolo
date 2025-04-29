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
            Some(Source::Local(path)) => Arc::new(LocalTileReader::new(path.clone())),
            Some(Source::S3 { .. }) => unimplemented!("S3 backend is not yet implemented"),
            None => anyhow::bail!("No source provided in the configuration"),
        };

        Ok(Self { config, reader })
    }

    pub async fn start(self) -> anyhow::Result<()> {
        let reader = self.reader;

        // Tile-serving router with state
        let app = Router::new()
            .route("/tiles/{layer}/{z}/{x}/{y}", get(tile_handler))
            .route("/layers", get(get_all_layers))
            .route("/map", get(webmap_handler))
            .with_state(reader.to_owned());

        let addr = SocketAddr::from(([0, 0, 0, 0], self.config.port));
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();

        // Choose a random layer for the example URL
        let layers = reader.list_layers().await;
        let random_layer = layers.keys().next().unwrap();

        println!("🚀 TileYolo serving on {}", addr);
        println!(
            "🗺️ QGIS XYZ-tiles path on random layer: http://{}/tiles/{}/{{z}}/{{x}}/{{y}}",
            addr, random_layer
        );
        println!("🌍 Browse layers visually at: http://{}/map", addr);
        println!("📚 Get all layers at: http://{}/layers", addr);

        axum::serve(listener, app.into_make_service())
            .await
            .unwrap();

        Ok(())
    }
}
