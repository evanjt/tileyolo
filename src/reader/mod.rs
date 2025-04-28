use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;

pub mod cog;
pub mod local;
pub mod s3;
pub mod style;

pub struct TileResponse {
    pub bytes: Vec<u8>,
    pub content_type: String,
}

#[derive(Debug, Clone)]
pub struct ColourStop {
    pub value: f32,
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub alpha: u8,
}

#[derive(Debug, Clone)]
pub struct Layer {
    pub layer: String,
    pub style: String,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub geometry: LayerGeometry,
    pub colour_stops: Vec<ColourStop>,
    pub min_value: f32,
    pub max_value: f32,
}

#[derive(Debug, Clone)]
pub struct LayerGeometry {
    pub crs_name: String,
    pub crs_code: i32,
}

#[async_trait]
pub trait TileReader: Send + Sync {
    async fn list_layers(&self) -> HashMap<String, Vec<String>>;
    async fn get_tile(
        &self,
        layer: &str,
        z: u8,
        x: u32,
        y: u32,
        style: Option<&str>,
    ) -> Result<TileResponse, String>;
}
