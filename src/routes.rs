use crate::reader::TileReader;
use axum::extract::State;
use axum::response::IntoResponse;
use axum::{extract::Path, http::StatusCode};
use std::sync::Arc;

pub async fn tile_handler(
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
