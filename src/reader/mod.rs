use crate::utils::geometry::{lon_lat_to_mercator, mercator_to_lon_lat};
use async_trait::async_trait;
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
    pub extent: GeometryExtent,
}

#[derive(Debug, Clone, Serialize)]
pub struct GeometryExtent {
    pub minx: f64,
    pub miny: f64,
    pub maxx: f64,
    pub maxy: f64,
}

impl From<(f64, f64, f64, f64)> for GeometryExtent {
    fn from(extent: (f64, f64, f64, f64)) -> Self {
        GeometryExtent {
            minx: extent.0, // minx
            miny: extent.1, // miny
            maxx: extent.2, // maxx
            maxy: extent.3, // maxy
        }
    }
}

impl LayerGeometry {
    /// Fast, branchless project between 4326 and 3857 with proj if needed for others
    pub fn project(&self, target_crs: i32) -> anyhow::Result<Self> {
        // no work if same
        if self.crs_code == target_crs {
            return Ok(self.clone());
        }

        let new_extent = match (self.crs_code, target_crs) {
            (4326, 3857) => {
                // lon/lat → WebMercator
                let (minx, miny) = lon_lat_to_mercator(self.extent.minx, self.extent.miny);
                let (maxx, maxy) = lon_lat_to_mercator(self.extent.maxx, self.extent.maxy);
                GeometryExtent::from((minx, miny, maxx, maxy))
            }
            (3857, 4326) => {
                // WebMercator → lon/lat
                let (minx, miny) = mercator_to_lon_lat(self.extent.minx, self.extent.miny);
                let (maxx, maxy) = mercator_to_lon_lat(self.extent.maxx, self.extent.maxy);
                GeometryExtent::from((minx, miny, maxx, maxy))
            }
            // any other CRS: fall back to PROJ
            _ => {
                let proj = proj::Proj::new_known_crs(
                    format!("EPSG:{}", self.crs_code).as_str(),
                    format!("EPSG:{}", target_crs).as_str(),
                    None,
                )
                .unwrap();
                let (minx, miny) = proj
                    .convert((self.extent.minx, self.extent.miny))
                    .map_err(anyhow::Error::from)?;
                let (maxx, maxy) = proj
                    .convert((self.extent.maxx, self.extent.maxy))
                    .map_err(anyhow::Error::from)?;
                GeometryExtent::from((minx, miny, maxx, maxy))
            }
        };

        Ok(LayerGeometry {
            crs_code: target_crs,
            extent: new_extent,
        })
    }

    /// Generate both 4326 & 3857 variants **synchronously** (no spawn_blocking!)
    pub fn generate_cached_geometry_sync(&self) -> anyhow::Result<HashMap<i32, LayerGeometry>> {
        let mut cache = HashMap::new();
        // always insert the source
        cache.insert(self.crs_code, self.clone());

        // compute the other one (or both if neither)
        let targets = if self.crs_code == 4326 {
            vec![3857]
        } else if self.crs_code == 3857 {
            vec![4326]
        } else {
            vec![4326, 3857]
        };

        for &t in &targets {
            let g = self.project(t)?;
            cache.insert(t, g);
        }

        Ok(cache)
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
