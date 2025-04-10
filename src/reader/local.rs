use super::{LayerStyle, TileReader, TileResponse};
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;
use walkdir::WalkDir;

pub struct LocalTileReader {
    root: PathBuf,
    layers: HashMap<String, Vec<LayerStyle>>, // layer -> styles
}

impl LocalTileReader {
    pub fn new(root: PathBuf) -> Self {
        let mut layers: HashMap<String, Vec<LayerStyle>> = HashMap::new();

        for entry in WalkDir::new(&root).min_depth(2).into_iter().flatten() {
            if let Some(ext) = entry.path().extension() {
                if ext == "tif" || ext == "tiff" {
                    if let Some(file_name) = entry.path().file_stem().and_then(|s| s.to_str()) {
                        if let Some(style) = entry
                            .path()
                            .parent()
                            .and_then(|p| p.file_name())
                            .and_then(|s| s.to_str())
                        {
                            let layer_style = LayerStyle {
                                layer: file_name.to_string(),
                                style: style.to_string(),
                                path: entry.clone().into_path(),
                            };
                            layers
                                .entry(file_name.to_string())
                                .or_default()
                                .push(layer_style);
                        }
                    }
                }
            }
        }

        // Warn about duplicate layer names across styles
        for (layer, styles) in &layers {
            if styles.len() > 1 {
                let style_names: Vec<_> = styles.iter().map(|s| s.style.clone()).collect();
                println!(
                    "⚠️  Layer '{}' has multiple styles: {:?}",
                    layer, style_names
                );
            }
        }

        Self { root, layers }
    }
}

#[async_trait]
impl TileReader for LocalTileReader {
    async fn list_layers(&self) -> HashMap<String, Vec<String>> {
        let mut result = HashMap::new();
        for (layer, styles) in &self.layers {
            result.insert(
                layer.clone(),
                styles.iter().map(|s| s.style.clone()).collect(),
            );
        }
        result
    }

    // #[axum::debug_handler]
    async fn get_tile(
        &self,
        layer: String,
        _z: u8,
        _x: u32,
        _y: u32,
        style: Option<String>,
    ) -> Result<TileResponse, String> {
        // Example implementation: Replace this with your actual logic to fetch the tile
        let tile_path = self
            .layers
            .get(&layer)
            .and_then(|styles| {
                styles.iter().find(|s| {
                    style
                        .as_ref()
                        .map(|style_name| &s.style == style_name)
                        .unwrap_or(true)
                })
            })
            .map(|s| s.path.clone())
            .ok_or_else(|| {
                format!(
                    "Tile not found for layer '{}' and style '{:?}'",
                    layer, style
                )
            })?;

        let tile_bytes = std::fs::read(tile_path).map_err(|e| e.to_string())?;
        let tile_response = TileResponse {
            content_type: "image/tiff".to_string(),
            bytes: tile_bytes,
        };

        Ok(tile_response)
    }
}
