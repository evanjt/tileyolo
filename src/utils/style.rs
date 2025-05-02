use crate::reader::ColourStop;
use colorgrad::{Gradient, preset};
use std::fs;
use std::path::Path;

pub fn parse_style_file<P: AsRef<Path>>(path: P) -> Result<Vec<ColourStop>, String> {
    let content =
        fs::read_to_string(path).map_err(|e| format!("Failed to read style.txt: {}", e))?;
    let mut stops = Vec::new();

    for line in content.lines() {
        if line.starts_with('#') || line.starts_with("INTERPOLATION") || line.trim().is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() < 5 {
            continue;
        }

        let value = parts[0]
            .parse()
            .map_err(|e| format!("Invalid value: {}", e))?;
        let red = parts[1]
            .parse()
            .map_err(|e| format!("Invalid red: {}", e))?;
        let green = parts[2]
            .parse()
            .map_err(|e| format!("Invalid green: {}", e))?;
        let blue = parts[3]
            .parse()
            .map_err(|e| format!("Invalid blue: {}", e))?;
        let alpha = parts[4]
            .parse()
            .map_err(|e| format!("Invalid alpha: {}", e))?;

        stops.push(ColourStop {
            value,
            red,
            green,
            blue,
            alpha,
        });
    }

    Ok(stops)
}

pub fn is_builtin_palette(name: &str) -> bool {
    matches!(
        name,
        "viridis"
            | "magma"
            | "plasma"
            | "inferno"
            | "turbo"
            | "cubehelix_default"
            | "rainbow"
            | "spectral"
            | "sinebow"
    )
}

pub fn get_builtin_gradient(name: &str) -> Option<Box<dyn Gradient>> {
    Some(match name {
        "viridis" => Box::new(preset::viridis()),
        "magma" => Box::new(preset::magma()),
        "plasma" => Box::new(preset::plasma()),
        "inferno" => Box::new(preset::inferno()),
        "turbo" => Box::new(preset::turbo()),
        "cubehelix_default" => Box::new(preset::cubehelix_default()),
        "rainbow" => Box::new(preset::rainbow()),
        "spectral" => Box::new(preset::spectral()),
        "sinebow" => Box::new(preset::sinebow()),
        _ => return None,
    })
}
