// src/reader/metadata.rs

use crate::{
    reader::{ColourStop, GeometryExtent, Layer, LayerGeometry},
    utils::style::{is_builtin_palette, parse_style_file},
};
use csv::{ReaderBuilder, WriterBuilder};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::Path,
    time::{Duration, UNIX_EPOCH},
};

/// The minimal fields we need to cache on disk.
#[derive(Serialize, Deserialize, Clone)]
pub struct LayerMetadata {
    pub layer: String,
    pub size_bytes: u64,
    pub last_modified: u64,
    pub crs_code: i32,
    pub min_value: f32,
    pub max_value: f32,
    pub is_cog: bool,

    // split extent tuple into four CSV columns
    pub extent_minx: f64,
    pub extent_miny: f64,
    pub extent_maxx: f64,
    pub extent_maxy: f64,
}

impl LayerMetadata {
    /// Build LayerMetadata from a full Layer
    pub fn from_layer(layer: &Layer) -> Self {
        let last_modified = layer
            .last_modified
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        LayerMetadata {
            layer: layer.layer.clone(),
            size_bytes: layer.size_bytes,
            last_modified,
            crs_code: layer.source_geometry.crs_code,
            min_value: layer.min_value,
            max_value: layer.max_value,
            is_cog: layer.is_cog,
            extent_minx: layer.source_geometry.extent.minx,
            extent_miny: layer.source_geometry.extent.miny,
            extent_maxx: layer.source_geometry.extent.maxx,
            extent_maxy: layer.source_geometry.extent.maxy,
        }
    }

    /// Reconstruct a Layer (including style/colour_stops) from metadata + actual file path
    pub fn to_layer(&self, path: &Path) -> Layer {
        // Style is determined at runtime from the immediate parent folder
        let style_name = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("default");

        let colour_stops: Vec<ColourStop> = if is_builtin_palette(style_name) {
            Vec::new()
        } else {
            let style_path = path.parent().unwrap().join("style.txt");
            parse_style_file(&style_path).unwrap_or_default()
        };

        let last_modified = UNIX_EPOCH + Duration::from_secs(self.last_modified);

        Layer {
            layer: self.layer.clone(),
            style: style_name.to_string(),
            path: path.to_path_buf(),
            size_bytes: self.size_bytes,
            source_geometry: LayerGeometry {
                crs_code: self.crs_code,
                extent: GeometryExtent {
                    minx: self.extent_minx,
                    miny: self.extent_miny,
                    maxx: self.extent_maxx,
                    maxy: self.extent_maxy,
                },
            },
            cached_geometry: HashMap::new(),
            colour_stops,
            min_value: self.min_value,
            max_value: self.max_value,
            is_cog: self.is_cog,
            last_modified,
        }
    }
}

pub type MetadataCache = HashMap<String, LayerMetadata>;

/// Load the metadata cache from disk (or return empty on any error)
pub fn load_cache(cache_path: &Path) -> MetadataCache {
    let mut cache = MetadataCache::new();
    if let Ok(mut rdr) = ReaderBuilder::new().has_headers(true).from_path(cache_path) {
        for meta in rdr.deserialize::<LayerMetadata>().flatten() {
            cache.insert(meta.layer.clone(), meta);
        }
    }
    cache
}

/// Save the metadata cache back to disk (ignore errors)
pub fn save_cache(cache_path: &Path, cache: &MetadataCache) {
    if let Ok(mut wtr) = WriterBuilder::new().has_headers(true).from_path(cache_path) {
        for meta in cache.values() {
            let _ = wtr.serialize(meta);
        }
        let _ = wtr.flush();
    }
}

/// Produce the key for a file (filename without extension only)
pub fn key_for(path: &Path, _root: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string()
}
