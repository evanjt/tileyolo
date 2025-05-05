use crate::reader::{LayerGeometry, TileReader};
use crate::{map::INDEX_HTML, reader::Layer};
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
    geometry: Vec<LayerGeometry>,
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
    let layers: Vec<Layer> = reader.list_layers().await;
    let mut all_layers: Vec<LayerResponse> = Vec::new();

    for layer in layers {
        {
            all_layers.push(LayerResponse {
                layer: layer.layer.clone(),
                style: layer.style.clone(),
                geometry: vec![layer.geometry.clone()],
            });
        }
    }

    // Sort the layers alphabetically case insensitive
    all_layers.sort_by(|a, b| {
        a.layer
            .to_lowercase()
            .cmp(&b.layer.to_lowercase())
            .then(a.style.to_lowercase().cmp(&b.style.to_lowercase()))
    });

    (StatusCode::OK, Json(all_layers))
}
