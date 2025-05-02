use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum Source {
    Local(PathBuf),
    S3 { bucket: String, prefix: String },
}

#[derive(Debug, Clone)]
pub struct Config {
    pub source: Option<Source>,
    pub data_folder: String,
    pub default_style: Option<String>,
    pub tile_size_x: u32,
    pub tile_size_y: u32,
    pub port: u16,
    pub default_raster_band: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            source: None,
            // Default may be S3 in the future...
            data_folder: "data".to_string(),
            default_style: Some("default".to_string()),
            tile_size_x: 256,
            tile_size_y: 256,
            port: 8000,
            default_raster_band: 1,
        }
    }
}

impl Config {
    pub fn parse_path_to_absolute(path: &PathBuf) -> PathBuf {
        // Convert the path to an absolute path
        let path = PathBuf::from(path);
        if path.is_absolute() {
            path
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path)
        }
    }

    pub fn default_data_folder() -> String {
        // Render the config data_folder as a string with the current path
        // to form an absolute path
        let default_data_dir = Self::default().data_folder.clone();
        let path = PathBuf::from(default_data_dir);
        Self::parse_path_to_absolute(&path)
            .to_string_lossy()
            .into_owned()
    }

    pub fn default_port() -> u16 {
        // Return the default port
        Self::default().port
    }
}
