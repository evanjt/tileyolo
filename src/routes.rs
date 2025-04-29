use crate::map::INDEX_HTML;
use crate::reader::TileReader;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse},
};
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
struct LayerResponse {
    layer: String,
    style: String,
}

pub(super) async fn webmap_handler() -> impl IntoResponse {
    Html(INDEX_HTML)
}

pub(super) async fn tile_handler(
    Path((layer, z, x, y)): Path<(String, u8, u32, u32)>,
    State(reader): State<Arc<dyn TileReader>>,
) -> impl IntoResponse {
    match reader.get_tile(&layer, z, x, y, None).await {
        Ok(tile) => axum::http::Response::builder()
            .header("Content-Type", tile.content_type)
            .body(axum::body::Body::from(tile.bytes))
            .unwrap()
            .into_response(),
        Err(e) => (StatusCode::NOT_FOUND, e).into_response(),
    }
}

pub(super) async fn get_all_layers(State(reader): State<Arc<dyn TileReader>>) -> impl IntoResponse {
    let layers: std::collections::HashMap<String, Vec<String>> = reader.list_layers().await;
    let mut all_layers: Vec<LayerResponse> = Vec::new();

    for (layer, styles) in layers {
        for style in styles {
            all_layers.push(LayerResponse {
                layer: layer.clone(),
                style: style.clone(),
            });
        }
    }

    (StatusCode::OK, Json(all_layers))
}
