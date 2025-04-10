use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;
pub mod local;
pub mod s3;

pub struct TileResponse {
    pub bytes: Vec<u8>,
    pub content_type: String,
}

#[derive(Debug, Clone)]
pub struct LayerStyle {
    pub layer: String,
    pub style: String,
    pub path: PathBuf,
}

#[async_trait]
pub trait TileReader: Send + Sync {
    async fn list_layers(&self) -> HashMap<String, Vec<String>>;
    async fn get_tile(
        &self,
        layer: String,
        z: u8,
        x: u32,
        y: u32,
        style: Option<String>,
    ) -> Result<TileResponse, String>;
}
