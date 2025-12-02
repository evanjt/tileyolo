use crate::error::TileYoloError;
use std::path::PathBuf;

/// Data source for tile serving.
#[derive(Debug, Clone)]
pub enum Source {
    /// Local filesystem directory containing GeoTIFF files organized by style folders.
    Local(PathBuf),
    /// S3-compatible storage bucket.
    S3 {
        /// Bucket name (e.g., "my-tiles-bucket")
        bucket: String,
        /// Key prefix within the bucket (e.g., "data/geotiffs")
        prefix: String,
    },
}

/// Configuration for TileYolo server.
///
/// Use [`ConfigBuilder`] for a fluent API to construct configurations.
///
/// # Example
///
/// ```rust
/// use tileyolo::{Config, Source};
/// use std::path::PathBuf;
///
/// let config = Config::builder()
///     .source(Source::Local(PathBuf::from("./data")))
///     .port(8080)
///     .build()
///     .expect("valid config");
/// ```
#[derive(Debug, Clone)]
pub struct Config {
    /// Data source (required for server operation)
    pub source: Option<Source>,
    /// Server port (1-65535)
    pub port: u16,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            source: None,
            port: 8000,
        }
    }
}

impl Config {
    /// Create a new configuration builder.
    #[must_use]
    pub fn builder() -> ConfigBuilder {
        ConfigBuilder::default()
    }

    /// Convert a path to an absolute path.
    #[must_use]
    pub fn parse_path_to_absolute(path: &PathBuf) -> PathBuf {
        if path.is_absolute() {
            path.clone()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path)
        }
    }

    /// Get the default data folder path (absolute).
    #[must_use]
    pub fn default_data_folder() -> String {
        Self::parse_path_to_absolute(&PathBuf::from("data"))
            .to_string_lossy()
            .into_owned()
    }

    /// Get the default port.
    #[must_use]
    pub fn default_port() -> u16 {
        8000
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), TileYoloError> {
        if self.port == 0 {
            return Err(TileYoloError::Config("Port cannot be 0".to_string()));
        }
        if self.source.is_none() {
            return Err(TileYoloError::Config(
                "No data source specified".to_string(),
            ));
        }
        Ok(())
    }
}

/// Builder for [`Config`] with validation.
///
/// # Example
///
/// ```rust
/// use tileyolo::{Config, Source};
/// use std::path::PathBuf;
///
/// let config = Config::builder()
///     .source(Source::Local(PathBuf::from("./geotiffs")))
///     .port(3000)
///     .build()
///     .expect("valid config");
/// ```
#[derive(Debug, Clone, Default)]
pub struct ConfigBuilder {
    source: Option<Source>,
    port: Option<u16>,
}

impl ConfigBuilder {
    /// Set the data source.
    #[must_use]
    pub fn source(mut self, source: Source) -> Self {
        self.source = Some(source);
        self
    }

    /// Set the server port (default: 8000).
    #[must_use]
    pub fn port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }

    /// Build and validate the configuration.
    ///
    /// # Errors
    ///
    /// Returns `TileYoloError::Config` if:
    /// - No source is specified
    /// - Port is 0
    pub fn build(self) -> Result<Config, TileYoloError> {
        let config = Config {
            source: self.source,
            port: self.port.unwrap_or(Config::default_port()),
        };
        config.validate()?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_with_local_source() {
        let config = Config::builder()
            .source(Source::Local(PathBuf::from("./data")))
            .port(8080)
            .build()
            .unwrap();

        assert_eq!(config.port, 8080);
        assert!(matches!(config.source, Some(Source::Local(_))));
    }

    #[test]
    fn test_builder_defaults() {
        let config = Config::builder()
            .source(Source::Local(PathBuf::from("./data")))
            .build()
            .unwrap();

        assert_eq!(config.port, 8000);
    }

    #[test]
    fn test_builder_missing_source_fails() {
        let result = Config::builder().port(8080).build();
        assert!(result.is_err());
    }

    #[test]
    fn test_builder_zero_port_fails() {
        let result = Config::builder()
            .source(Source::Local(PathBuf::from("./data")))
            .port(0)
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_path_to_absolute() {
        let relative = PathBuf::from("data");
        let absolute = Config::parse_path_to_absolute(&relative);
        assert!(absolute.is_absolute());
    }

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.port, 8000);
        assert!(config.source.is_none());
    }
}
