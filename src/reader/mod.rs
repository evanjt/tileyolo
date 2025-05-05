use async_trait::async_trait;
use proj::Proj;
use serde::Serialize;
use std::{collections::HashMap, path::PathBuf};

pub mod cog;
pub mod local;
pub mod metadata;
pub mod s3;

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
    pub source_geometry: LayerGeometry,
    pub cached_geometry: HashMap<i32, LayerGeometry>, // Used to cache the projected extents for supplying endpoint
    pub colour_stops: Vec<ColourStop>,
    pub min_value: f32,
    pub max_value: f32,
    pub is_cog: bool,
    pub last_modified: std::time::SystemTime,
}

#[derive(Debug, Clone, Serialize)]
pub struct LayerGeometry {
    pub crs_code: i32,
    pub extent: (f64, f64, f64, f64), // (minx, miny, maxx, maxy)
}

impl LayerGeometry {
    pub fn project(&self, target_crs: i32) -> anyhow::Result<Self> {
        if self.crs_code == target_crs {
            return Ok(self.clone());
        }

        let proj = Proj::new_known_crs(
            format!("EPSG:{}", self.crs_code).as_str(),
            format!("EPSG:{}", target_crs).as_str(),
            None,
        )
        .unwrap();

        let (minx, miny) = proj
            .convert((self.extent.0, self.extent.1))
            .map_err(anyhow::Error::from)?;
        let (maxx, maxy) = proj
            .convert((self.extent.2, self.extent.3))
            .map_err(anyhow::Error::from)?;

        Ok(LayerGeometry {
            crs_code: target_crs,
            extent: (minx, miny, maxx, maxy),
        })
    }
}
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
