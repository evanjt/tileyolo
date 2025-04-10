use super::ColorStop;
use std::fs;
use std::path::Path;

pub fn parse_style_file<P: AsRef<Path>>(path: P) -> Result<Vec<ColorStop>, String> {
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

        stops.push(ColorStop {
            value,
            red,
            green,
            blue,
            alpha,
        });
    }

    Ok(stops)
}
