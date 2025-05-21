use crate::endpoints::map::INDEX_HTML;
use crate::endpoints::server::AppState;
use crate::models::layer::{Layer, LayerGeometry};
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, State, WebSocketUpgrade};
use axum::response::{Html, IntoResponse, Response};
use axum::{Json, http::StatusCode};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Serialize)]
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
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    match state.reader.get_tile(&layer, z, x, y, None).await {
        Ok(tile) => axum::http::Response::builder()
            .header("Content-Type", tile.content_type)
            .body(axum::body::Body::from(tile.bytes))
            .unwrap()
            .into_response(),
        Err(e) => (StatusCode::NOT_FOUND, e).into_response(),
    }
}

pub async fn get_all_layers(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let layers: Vec<Layer> = state.reader.list_layers().await;
    let mut all_layers: Vec<LayerResponse> = Vec::new();
    for layer in layers {
        all_layers.push(LayerResponse {
            layer: layer.layer.clone(),
            style: layer.style.clone(),
            geometry: layer.cached_geometry,
        });
    }
    all_layers.sort_by(|a, b| {
        a.layer
            .to_lowercase()
            .cmp(&b.layer.to_lowercase())
            .then(a.style.to_lowercase().cmp(&b.style.to_lowercase()))
    });
    (StatusCode::OK, Json(all_layers)).into_response()
}

// Serve the stats dashboard HTML
pub async fn stats_dashboard() -> impl IntoResponse {
    Html(
        r#"
    <!DOCTYPE html>
    <html lang='en'>
    <head>
        <meta charset='UTF-8'>
        <title>TileYolo Stats</title>
        <style>
            body { font-family: sans-serif; background: #181818; color: #eee; }
            .bar { height: 24px; background: #333; border-radius: 8px; margin: 8px 0; }
            .fill { height: 100%; background: #09c; border-radius: 8px; transition: width 0.3s; }
            .label { position: absolute; left: 12px; top: 2px; font-size: 16px; }
            .section { margin-bottom: 24px; }
        </style>
    </head>
    <body>
        <h1>TileYolo Stats</h1>
        <div id='stats'></div>
        <script>
        const statsDiv = document.getElementById('stats');
        let ws = new WebSocket(`ws://${location.host}/stats/ws`);
        ws.onmessage = (event) => {
            const s = JSON.parse(event.data);
            statsDiv.innerHTML = `
                <div class='section'>
                    <b>Cache Usage:</b><br>
                    <div class='bar' style='position:relative;width:400px;'>
                        <div class='fill' style='width:${s.cache_percent*100}%;'></div>
                        <span class='label'>${s.cache_size} / ${s.max_cache} tiles</span>
                    </div>
                </div>
                <div class='section'><b>Cache Growth:</b> ${s.growth_str}</div>
                <div class='section'><b>Serving Speed:</b> ${s.speed.toFixed(1)} tiles/sec</div>
                <div class='section'>
                    <b>Cache Efficiency:</b><br>
                    <div class='bar' style='position:relative;width:400px;'>
                        <div class='fill' style='width:${s.efficiency*100}%;background:#3c3;'></div>
                        <span class='label'>${(s.efficiency*100).toFixed(1)}% hits</span>
                    </div>
                </div>
                <div class='section'><b>RAM Recommendation:</b> ${s.ram_recommend ? '⚠️ Consider increasing RAM for better cache efficiency!' : 'RAM is sufficient for current workload.'}</div>
            `;
        };
        </script>
    </body>
    </html>
    "#,
    )
}

// WebSocket endpoint for live stats
pub async fn stats_ws(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(move |socket| stats_ws_stream(socket, state))
}

async fn stats_ws_stream(mut socket: WebSocket, state: Arc<AppState>) {
    let mut prev_cache_size = 0u64;
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
    loop {
        interval.tick().await;
        let cache_size = state
            .cache
            .as_ref()
            .map(|c| c.entry_count() as u64)
            .unwrap_or(0);
        let growth_trend = cache_size as i64 - prev_cache_size as i64;
        prev_cache_size = cache_size;
        let cache_percent = if state.max_cache > 0 {
            (cache_size as f64 / state.max_cache as f64).min(1.0)
        } else {
            0.0
        };
        let (hit, miss, efficiency) = if let Some(stats) = &state.stats {
            let hit = stats.cache_hits.load(std::sync::atomic::Ordering::Relaxed);
            let miss = stats
                .cache_misses
                .load(std::sync::atomic::Ordering::Relaxed);
            let total = hit + miss;
            let efficiency = if total > 0 {
                hit as f64 / total as f64
            } else {
                1.0
            };
            (hit, miss, efficiency)
        } else {
            (0, 0, 1.0)
        };
        let speed = state.stats.as_ref().map(|s| s.speed()).unwrap_or(0.0);
        let ram_recommend = cache_percent > 0.9 && efficiency < 0.7;
        let growth_str = if growth_trend > 0 {
            format!("Growing (+{} tiles)", growth_trend)
        } else if growth_trend < 0 {
            format!("Shrinking ({} tiles)", growth_trend)
        } else {
            "Stable".to_string()
        };
        let msg = serde_json::json!({
            "cache_size": cache_size,
            "max_cache": state.max_cache,
            "cache_percent": cache_percent,
            "growth_str": growth_str,
            "speed": speed,
            "efficiency": efficiency,
            "ram_recommend": ram_recommend
        });
        if socket
            .send(Message::Text(msg.to_string().into()))
            .await
            .is_err()
        {
            break;
        }
    }
}
