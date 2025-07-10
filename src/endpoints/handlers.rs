use crate::endpoints::map::INDEX_HTML;
use crate::models::layer::{Layer, LayerGeometry};
use crate::traits::TileReader;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse},
};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Serialize, serde::Deserialize)]
struct LayerResponse {
    layer: String,
    style: String,
    geometry: HashMap<i32, LayerGeometry>,
}

pub async fn webmap_handler() -> impl IntoResponse {
    Html(INDEX_HTML)
}

pub async fn tile_handler(
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

pub async fn get_all_layers(State(reader): State<Arc<dyn TileReader>>) -> impl IntoResponse {
    let layers: Vec<Layer> = reader.list_layers().await;
    let mut all_layers: Vec<LayerResponse> = Vec::new();

    for layer in layers {
        {
            all_layers.push(LayerResponse {
                layer: layer.layer.clone(),
                style: layer.style.clone(),
                geometry: layer.cached_geometry, // Assumes this has already been populated at load
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::test_helpers::*;
    use crate::reader::local::LocalTileReader;
    use axum::body::to_bytes;
    use axum::http::{Request, StatusCode};
    use tower::util::ServiceExt;
    
    #[tokio::test]
    async fn test_get_all_layers_endpoint() {
        // Setup test data
        let (tmp_dir, data_dir) = setup_test_data_dir();
        
        // Create test GeoTIFF files in different style directories
        create_test_geotiff(
            &data_dir.join("viridis"),
            "elevation.tif",
            (100, 100),
            4326,
            (-180.0, 180.0, -90.0, 90.0),
            (0.0, 1000.0),
            0.1,
            42,
        );
        
        create_test_geotiff(
            &data_dir.join("magma"),
            "temperature.tif",
            (100, 100),
            4326,
            (-180.0, 180.0, -90.0, 90.0),
            (-50.0, 50.0),
            0.05,
            43,
        );
        
        // Create reader and handler
        let reader = Arc::new(LocalTileReader::new(&data_dir).await) as Arc<dyn TileReader>;
        let app = crate::endpoints::server::create_router(reader.clone());
        
        // Make request
        let response = app
            .oneshot(Request::builder().uri("/layers").body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap();
        
        assert_eq!(response.status(), StatusCode::OK);
        
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let layers: Vec<LayerResponse> = serde_json::from_slice(&body).unwrap();
        
        // Verify response
        assert_eq!(layers.len(), 2);
        assert!(layers.iter().any(|l| l.layer == "elevation" && l.style == "viridis"));
        assert!(layers.iter().any(|l| l.layer == "temperature" && l.style == "magma"));
        
        // Verify alphabetical sorting
        assert_eq!(layers[0].layer, "elevation");
        assert_eq!(layers[1].layer, "temperature");
        
        drop(tmp_dir);
    }
    
    #[tokio::test]
    async fn test_tile_handler_valid_request() {
        let (tmp_dir, data_dir) = setup_test_data_dir();
        
        // Create test data with known Web Mercator bounds
        create_test_geotiff(
            &data_dir.join("grayscale"),
            "world.tif",
            (256, 256),
            3857,
            (-20037508.34, 20037508.34, -20037508.34, 20037508.34),
            (0.0, 255.0),
            0.0,
            44,
        );
        
        let reader = Arc::new(LocalTileReader::new(&data_dir).await) as Arc<dyn TileReader>;
        let app = crate::endpoints::server::create_router(reader.clone());
        
        // Request tile at zoom 0
        let response = app
            .oneshot(Request::builder()
                .uri("/tiles/world/0/0/0")
                .body(axum::body::Body::empty())
                .unwrap())
            .await
            .unwrap();
        
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers().get("Content-Type").unwrap(), "image/png");
        
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(!body.is_empty());
        
        // Verify it's a valid PNG
        assert_eq!(&body[0..8], b"\x89PNG\r\n\x1a\n");
        
        drop(tmp_dir);
    }
    
    #[tokio::test]
    async fn test_tile_handler_invalid_layer() {
        let (tmp_dir, data_dir) = setup_test_data_dir();
        let reader = Arc::new(LocalTileReader::new(&data_dir).await) as Arc<dyn TileReader>;
        let app = crate::endpoints::server::create_router(reader.clone());
        
        let response = app
            .oneshot(Request::builder()
                .uri("/tiles/nonexistent/0/0/0")
                .body(axum::body::Body::empty())
                .unwrap())
            .await
            .unwrap();
        
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        drop(tmp_dir);
    }
    
    #[tokio::test]
    async fn test_webmap_handler() {
        let (tmp_dir, data_dir) = setup_test_data_dir();
        let reader = Arc::new(LocalTileReader::new(&data_dir).await) as Arc<dyn TileReader>;
        let app = crate::endpoints::server::create_router(reader.clone());
        
        let response = app
            .oneshot(Request::builder()
                .uri("/map")
                .body(axum::body::Body::empty())
                .unwrap())
            .await
            .unwrap();
        
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers().get("Content-Type").unwrap(), "text/html; charset=utf-8");
        
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("leaflet"));
        
        drop(tmp_dir);
    }
}
