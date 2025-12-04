use crate::models::{geometry::GeometryExtent, style::ColourStop};
use geocog::project_point;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::PathBuf};

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
    /// Number of bands/channels in the image (1=grayscale, 3=RGB, 4=RGBA)
    pub bands: usize,
    /// Whether the file is tiled (COG-optimised) or stripped (not optimised)
    /// Tiled files are faster for partial reads and cloud access
    pub is_tiled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerGeometry {
    pub crs_code: i32,
    pub extent: GeometryExtent,
}

impl LayerGeometry {
    /// Fast, branchless project between 4326 and 3857 with proj if needed for others
    pub fn project(&self, target_crs: i32) -> anyhow::Result<Self> {
        // no work if same
        if self.crs_code == target_crs {
            return Ok(self.clone());
        }

        // Use geocog's unified project_point for all CRS transformations
        let (minx, miny) = project_point(self.crs_code, target_crs, self.extent.minx, self.extent.miny)
            .map_err(anyhow::Error::msg)?;
        let (maxx, maxy) = project_point(self.crs_code, target_crs, self.extent.maxx, self.extent.maxy)
            .map_err(anyhow::Error::msg)?;
        let new_extent = GeometryExtent::from((minx, miny, maxx, maxy));

        Ok(LayerGeometry {
            crs_code: target_crs,
            extent: new_extent,
        })
    }

    /// Generate both 4326 & 3857 variants for caching
    pub fn generate_cached_geometry_sync(&self) -> anyhow::Result<HashMap<i32, LayerGeometry>> {
        let mut cache = HashMap::new();
        // always insert the source
        cache.insert(self.crs_code, self.clone());

        // compute the other one (or both if neither)
        let targets: Vec<i32> = if self.crs_code == 4326 {
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
