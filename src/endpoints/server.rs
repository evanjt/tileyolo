use crate::config::{Config, Source};
use crate::endpoints::handlers::{
    get_all_layers, stats_dashboard, stats_ws, tile_handler, webmap_handler,
};
use crate::reader::local::LocalTileReader;
use crate::traits::TileReader;
use axum::{Router, routing::get};
use std::net::SocketAddr;
use std::sync::Arc;

pub struct AppState {
    pub reader: Arc<dyn TileReader>,
    pub cache: Option<Arc<moka::future::Cache<crate::reader::local::TileCacheKey, Arc<Vec<u8>>>>>,
    pub stats: Option<crate::utils::status::Stats>,
    pub max_cache: u64,
}

pub struct TileServer {
    config: Config,
    state: AppState,
}

impl TileServer {
    pub async fn new(config: Config) -> anyhow::Result<Self> {
        let cache_size_bytes = (config.cache_size_gb as u64) * 1024 * 1024 * 1024;
        let stats = crate::utils::status::Stats::default();
        match &config.source {
            Some(Source::Local(path)) => {
                let reader = LocalTileReader::new(path, cache_size_bytes, stats.clone()).await;
                let cache = reader.tile_cache.clone();
                let state = AppState {
                    reader: Arc::new(reader),
                    cache: Some(cache),
                    stats: Some(stats),
                    max_cache: cache_size_bytes,
                };
                return Ok(Self { config, state });
            }
            Some(Source::S3 { .. }) => unimplemented!("S3 backend is not yet implemented"),
            None => anyhow::bail!("No source provided in the configuration"),
        }
    }

    pub async fn start(self) -> anyhow::Result<()> {
        let state = Arc::new(self.state);
        let app = Router::new()
            .route("/tiles/{layer}/{z}/{x}/{y}", get(tile_handler))
            .route("/layers", get(get_all_layers))
            .route("/map", get(webmap_handler))
            .route("/stats", get(stats_dashboard))
            .route("/stats/ws", get(stats_ws))
            .with_state(state.clone());
        let addr = SocketAddr::from(([0, 0, 0, 0], self.config.port));
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();

        // Choose a random layer for the example URL
        let layers = state.reader.list_layers().await;

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

    📊 Live cache stats dashboard
       → http://{}/stats
            "#,
            addr, random_layer, addr, random_layer, addr, addr, addr
        );

        axum::serve(listener, app).await.unwrap();

        Ok(())
    }
}
