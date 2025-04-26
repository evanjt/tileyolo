use crate::config::{Config, Source};
use crate::reader::TileReader;
use crate::reader::local::LocalTileReader;
use crate::routes::tile_handler;
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
        let reader: Arc<dyn TileReader> = self.reader;
        // Get all the layers from reader and list quantity
        let layers = reader.list_layers().await;
        println!("📦 Total layers: {}", layers.len());
        // println!("Layers {:?}", layers);
        let app: Router = Router::new()
            .route("/tiles/{layer}/{z}/{x}/{y}", get(tile_handler))
            .with_state(reader);

        let addr = SocketAddr::from(([0, 0, 0, 0], self.config.port));
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();

        // Choose a random layer from layers vector for example URL
        let random_layer = layers.keys().next().unwrap();
        println!(
            "🚀 TileYolo serving on {}. Example: http://{}/tiles/{}/{}/{}/{}",
            addr, addr, random_layer, 0, 0, 0
        );

        axum::serve(listener, app).await.unwrap();

        Ok(())
    }
}
