//! Error types for TileYolo operations.

use std::path::PathBuf;
use thiserror::Error;

/// Errors that can occur during tile server operations.
///
/// This error type provides specific variants for different failure modes,
/// allowing callers to handle errors appropriately (e.g., returning 404
/// for [`LayerNotFound`](TileYoloError::LayerNotFound) vs 500 for internal errors).
#[derive(Error, Debug)]
pub enum TileYoloError {
    /// The requested layer does not exist.
    #[error("Layer not found: {name}")]
    LayerNotFound {
        /// Name of the missing layer.
        name: String,
    },

    /// Invalid XYZ tile coordinates were requested.
    #[error("Invalid tile coordinates: z={z}, x={x}, y={y}")]
    InvalidTile {
        /// Zoom level.
        z: u8,
        /// X coordinate.
        x: u32,
        /// Y coordinate.
        y: u32,
    },

    /// The requested tile is outside the layer's geographic bounds.
    #[error("Tile outside layer bounds")]
    TileOutOfBounds,

    /// Failed to read from the data source (file, S3, etc.).
    #[error("Failed to read data source: {0}")]
    DataRead(String),

    /// Failed to process raster data (decompression, resampling, etc.).
    #[error("Failed to process raster data: {0}")]
    RasterProcessing(String),

    /// Failed to encode the tile as PNG.
    #[error("PNG encoding failed: {0}")]
    PngEncoding(String),

    /// Invalid configuration.
    #[error("Configuration error: {0}")]
    Config(String),

    /// File I/O error with path context.
    #[error("IO error: {path}: {message}")]
    Io {
        /// Path that caused the error.
        path: PathBuf,
        /// Error message.
        message: String,
    },

    /// Coordinate reference system transformation failed.
    #[error("Coordinate transformation failed: {0}")]
    CoordinateTransform(String),

    /// Requested feature is not supported.
    #[error("Unsupported feature: {0}")]
    Unsupported(String),
}

impl TileYoloError {
    /// Create an IO error with path context.
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
