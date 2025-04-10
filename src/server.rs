use crate::config::{Config, Source};
use crate::reader::TileReader;
use crate::reader::local::LocalTileReader;
use axum::extract::State;
use axum::response::IntoResponse;
use axum::{Router, extract::Path, http::StatusCode, routing::get};
use std::net::SocketAddr;
use std::sync::Arc;

pub struct TileServer {
    config: Config,
    reader: Arc<dyn TileReader>,
}

impl TileServer {
    pub fn new(config: Config) -> anyhow::Result<Self> {
        let reader: Arc<dyn TileReader> = match &config.source {
            Source::Local(path) => Arc::new(LocalTileReader::new(path.clone())),
            Source::S3 { .. } => unimplemented!("S3 backend is not yet implemented"),
        };

        Ok(Self { config, reader })
    }

    pub async fn start(self) -> anyhow::Result<()> {
        let reader = self.reader.clone();
        let app: Router = Router::new()
            .route("/tiles/{layer}/{z}/{x}/{y}", get(tile_handler))
            .with_state(reader);

        let addr = SocketAddr::from(([0, 0, 0, 0], self.config.port));
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        println!("🚀 TileYolo serving on {}", addr);
        axum::serve(listener, app).await.unwrap();

        Ok(())
    }
}

async fn tile_handler(
    Path((layer, z, x, y)): Path<(String, u8, u32, u32)>,
    State(reader): State<Arc<dyn TileReader>>,
) -> impl IntoResponse {
    match reader.get_tile(layer, z, x, y, None).await {
        Ok(tile) => axum::http::Response::builder()
            .header("Content-Type", tile.content_type)
            .body(axum::body::Body::from(tile.bytes))
            .unwrap()
            .into_response(),
        Err(e) => (StatusCode::NOT_FOUND, e).into_response(),
    }
}
