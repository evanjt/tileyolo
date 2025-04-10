use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum Source {
    Local(PathBuf),
    S3 { bucket: String, prefix: String },
}

#[derive(Debug, Clone)]
pub struct Config {
    pub source: Source,
    pub default_style: Option<String>,
    pub tile_size: u32,
    pub port: u16,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            source: Source::Local(PathBuf::from("data")),
            default_style: Some("default".to_string()),
            tile_size: 256,
            port: 8000,
        }
    }
}
