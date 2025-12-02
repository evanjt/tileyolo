use std::path::PathBuf;
use thiserror::Error;

/// Errors that can occur during tile server operations.
#[derive(Error, Debug)]
pub enum TileYoloError {
    #[error("Layer not found: {name}")]
    LayerNotFound { name: String },

    #[error("Invalid tile coordinates: z={z}, x={x}, y={y}")]
    InvalidTile { z: u8, x: u32, y: u32 },

    #[error("Tile outside layer bounds")]
    TileOutOfBounds,

    #[error("Failed to read data source: {0}")]
    DataRead(String),

    #[error("Failed to process raster data: {0}")]
    RasterProcessing(String),

    #[error("PNG encoding failed: {0}")]
    PngEncoding(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("IO error: {path}: {message}")]
    Io { path: PathBuf, message: String },

    #[error("Coordinate transformation failed: {0}")]
    CoordinateTransform(String),

    #[error("Unsupported feature: {0}")]
    Unsupported(String),
}

impl TileYoloError {
    pub fn io(path: impl Into<PathBuf>, err: impl std::fmt::Display) -> Self {
        Self::Io {
            path: path.into(),
            message: err.to_string(),
        }
    }
}

impl From<std::io::Error> for TileYoloError {
    fn from(err: std::io::Error) -> Self {
        Self::DataRead(err.to_string())
    }
}
