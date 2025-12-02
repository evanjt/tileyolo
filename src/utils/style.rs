use crate::models::style::ColourStop;
use colorgrad::{Gradient, preset};
use std::fs;
use std::path::Path;

pub fn parse_style_file<P: AsRef<Path>>(path: P) -> Result<Vec<ColourStop>, String> {
    let content =
        fs::read_to_string(path).map_err(|e| format!("Failed to read style.txt: {e}"))?;
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
            .map_err(|e| format!("Invalid value: {e}"))?;
        let red = parts[1]
            .parse()
            .map_err(|e| format!("Invalid red: {e}"))?;
        let green = parts[2]
            .parse()
            .map_err(|e| format!("Invalid green: {e}"))?;
        let blue = parts[3]
            .parse()
            .map_err(|e| format!("Invalid blue: {e}"))?;
        let alpha = parts[4]
            .parse()
            .map_err(|e| format!("Invalid alpha: {e}"))?;

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
    get_builtin_gradient(name).is_some() || is_rgb_style(name)
}

/// Check if the style name indicates RGB passthrough rendering
pub fn is_rgb_style(name: &str) -> bool {
    matches!(name, "rgb" | "RGB" | "rgba" | "RGBA" | "truecolor")
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_is_builtin_palette() {
        // Test known built-in palettes
        assert!(is_builtin_palette("viridis"));
        assert!(is_builtin_palette("magma"));
        assert!(is_builtin_palette("plasma"));
        assert!(is_builtin_palette("inferno"));
        assert!(is_builtin_palette("turbo"));
        assert!(is_builtin_palette("rainbow"));
        assert!(is_builtin_palette("spectral"));
        assert!(is_builtin_palette("sinebow"));

        // Test non-existent palettes
        assert!(!is_builtin_palette("not_a_palette"));
        assert!(!is_builtin_palette(""));
        assert!(!is_builtin_palette("custom_style"));
    }

    #[test]
    fn test_get_builtin_gradient() {
        // Test that we can get valid gradients
        assert!(get_builtin_gradient("viridis").is_some());
        assert!(get_builtin_gradient("plasma").is_some());

        // Test that invalid names return None
        assert!(get_builtin_gradient("invalid").is_none());
        assert!(get_builtin_gradient("").is_none());
    }

    #[test]
    fn test_gradient_produces_colors() {
        let viridis = get_builtin_gradient("viridis").unwrap();

        // Gradients should produce valid RGBA values at different positions
        let color_at_0 = viridis.at(0.0);
        let color_at_half = viridis.at(0.5);
        let color_at_1 = viridis.at(1.0);

        // Colors at different positions should be different
        assert_ne!(color_at_0.to_rgba8(), color_at_half.to_rgba8());
        assert_ne!(color_at_half.to_rgba8(), color_at_1.to_rgba8());
    }

    #[test]
    fn test_parse_style_file_valid() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "# Comment line").unwrap();
        writeln!(file, "INTERPOLATION LINEAR").unwrap();
        writeln!(file, "0,0,0,128,255").unwrap();
        writeln!(file, "100,255,0,0,255").unwrap();
        writeln!(file, "200,0,255,0,255").unwrap();
        file.flush().unwrap();

        let stops = parse_style_file(file.path()).unwrap();

        assert_eq!(stops.len(), 3);

        assert_eq!(stops[0].value, 0.0);
        assert_eq!(stops[0].red, 0);
        assert_eq!(stops[0].green, 0);
        assert_eq!(stops[0].blue, 128);
        assert_eq!(stops[0].alpha, 255);

        assert_eq!(stops[1].value, 100.0);
        assert_eq!(stops[1].red, 255);
        assert_eq!(stops[1].green, 0);
        assert_eq!(stops[1].blue, 0);

        assert_eq!(stops[2].value, 200.0);
        assert_eq!(stops[2].red, 0);
        assert_eq!(stops[2].green, 255);
        assert_eq!(stops[2].blue, 0);
    }

    #[test]
    fn test_parse_style_file_skips_invalid_lines() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "# Comment").unwrap();
        writeln!(file).unwrap();  // Empty line
        writeln!(file, "not,enough,columns").unwrap();  // Invalid line
        writeln!(file, "50,128,128,128,200").unwrap();  // Valid line
        file.flush().unwrap();

        let stops = parse_style_file(file.path()).unwrap();

        assert_eq!(stops.len(), 1);
        assert_eq!(stops[0].value, 50.0);
    }

    #[test]
    fn test_parse_style_file_empty() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "# Only comments").unwrap();
        writeln!(file, "INTERPOLATION LINEAR").unwrap();
        file.flush().unwrap();

        let stops = parse_style_file(file.path()).unwrap();

        assert!(stops.is_empty());
    }

    #[test]
    fn test_parse_style_file_nonexistent() {
        let result = parse_style_file("/nonexistent/path/style.txt");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_style_file_invalid_number() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "abc,0,0,128,255").unwrap();  // Invalid value
        file.flush().unwrap();

        let result = parse_style_file(file.path());
        assert!(result.is_err());
    }
}
