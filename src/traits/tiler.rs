use crate::models::{layer::Layer, responses::TileResponse};
use async_trait::async_trait;

#[async_trait]
pub trait TileReader: Send + Sync {
    async fn list_layers(&self) -> Vec<Layer>;
    async fn get_tile(
        &self,
        layer: &str,
        z: u8,
        x: u32,
        y: u32,
        style: Option<&str>,
    ) -> Result<TileResponse, String>;
}
