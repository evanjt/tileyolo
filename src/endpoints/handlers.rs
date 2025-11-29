use crate::endpoints::map::INDEX_HTML;
use crate::models::layer::{Layer, LayerGeometry};
use crate::traits::TileReader;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Serialize, Deserialize)]
pub struct TileRequest {
    layer: String,
    z: u8,
    x: u32,
    y: u32,
}

#[derive(Deserialize, Default)]
pub struct TileQuery {
    pub style: Option<String>,
}

pub async fn webmap_handler() -> impl IntoResponse {
    Html(INDEX_HTML)
}

pub async fn tile_handler(
    Path((layer, z, x, y)): Path<(String, u8, u32, u32)>,
    Query(query): Query<TileQuery>,
    State(reader): State<Arc<dyn TileReader>>,
) -> impl IntoResponse {
    match reader.get_tile(&layer, z, x, y, query.style.as_deref()).await {
        Ok(tile_data) => tile_data.into_response(),
        Err(e) => {
            eprintln!("Error generating tile: {:?}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Error generating tile").into_response()
        }
    }
}

#[derive(Serialize)]
pub struct LayerResponse {
    layer: String,
    style: String,
    path: String,
    size_bytes: u64,
    source_geometry: LayerGeometry,
    colour_stops: Vec<crate::models::style::ColourStop>,
    min_value: f32,
    max_value: f32,
    is_cog: bool,
    /// Whether the file is tiled (COG-optimised) or stripped (not optimised for streaming)
    is_tiled: bool,
    last_modified: std::time::SystemTime,
}

pub async fn get_all_layers(State(reader): State<Arc<dyn TileReader>>) -> impl IntoResponse {
    let layers: Vec<Layer> = reader.list_layers().await;
    let mut all_layers: Vec<LayerResponse> = Vec::new();

    for layer in layers {
        all_layers.push(LayerResponse {
            layer: layer.layer.clone(),
            style: layer.style.clone(),
            path: layer.path.to_string_lossy().to_string(),
            size_bytes: layer.size_bytes,
            source_geometry: layer.source_geometry,
            colour_stops: layer.colour_stops,
            min_value: layer.min_value,
            max_value: layer.max_value,
            is_cog: layer.is_cog,
            is_tiled: layer.is_tiled,
            last_modified: layer.last_modified,
        });
    }

    (StatusCode::OK, Json(all_layers))
}

// Test code temporarily disabled due to GDAL dependency
// TODO: Update to use cog3pio instead of GDAL
