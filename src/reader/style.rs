use super::ColourStop;
use colorgrad::{Gradient, preset};
use comfy_table::{Attribute, Cell, CellAlignment, Table};
use std::collections::HashMap;
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

pub fn print_style_summary(
    style_info: &HashMap<String, (usize, Vec<ColourStop>, f32, f32, usize)>,
) {
    let mut table = Table::new();
    table
        .set_header(vec![
            Cell::new("")
                .add_attribute(Attribute::Bold)
                .set_alignment(CellAlignment::Center),
            Cell::new("Style")
                .add_attribute(Attribute::Bold)
                .set_alignment(CellAlignment::Center),
            Cell::new("Layers")
                .add_attribute(Attribute::Bold)
                .set_alignment(CellAlignment::Center),
            Cell::new("Breaks")
                .add_attribute(Attribute::Bold)
                .set_alignment(CellAlignment::Center),
            Cell::new("Min")
                .add_attribute(Attribute::Bold)
                .set_alignment(CellAlignment::Center),
            Cell::new("Max")
                .add_attribute(Attribute::Bold)
                .set_alignment(CellAlignment::Center),
            Cell::new("Colourbar").add_attribute(Attribute::Bold),
        ])
        .load_preset(comfy_table::presets::ASCII_BORDERS_ONLY_CONDENSED);

    let mut warnings = Vec::new();

    for (style, (count, stops, min_v, max_v, num_cogs)) in style_info {
        let breaks_str = if is_builtin_palette(style) {
            "auto".to_string()
        } else {
            stops
                .iter()
                .map(|s| format!("{:.2}", s.value))
                .collect::<Vec<_>>()
                .join(", ")
        };

        let bar = if let Some(grad) = get_builtin_gradient(style) {
            let mut s = String::new();
            let n = 10;
            for i in 0..n {
                let t = i as f32 / (n - 1) as f32;
                let [r, g, b, _] = grad.at(t).to_rgba8();
                s.push_str(&format!("\x1b[38;2;{};{};{}m█\x1b[0m", r, g, b));
            }
            s
        } else {
            let mut s = String::new();
            for cs in stops {
                s.push_str(&format!(
                    "\x1b[38;2;{};{};{}m█\x1b[0m",
                    cs.red, cs.green, cs.blue
                ));
            }
            s
        };

        let mut style_row = vec![
            Cell::new("✅").set_alignment(CellAlignment::Center), // Default success overwritten to warning if needed
            Cell::new(style),
            Cell::new(*count).set_alignment(CellAlignment::Center),
            Cell::new(breaks_str).set_alignment(CellAlignment::Center),
            Cell::new(min_v).set_alignment(CellAlignment::Center),
            Cell::new(max_v).set_alignment(CellAlignment::Center),
            Cell::new(bar),
        ];

        if !stops.is_empty() {
            let style_min = stops.first().unwrap().value;
            let style_max = stops.last().unwrap().value;
            if *min_v < style_min || *max_v > style_max {
                warnings.push(format!(
                    "  ⚠️{}: Colour stops [{:.2}…{:.2}] do NOT cover data range [{:.2}…{:.2}]",
                    style, style_min, style_max, min_v, max_v
                ));
                style_row[0] = Cell::new("⚠️");
            }
        }

        if num_cogs < count {
            warnings.push(format!(
                "  ⚠️{}: {} of {} layers are COGs, performance will be degraded on large datasets",
                style, num_cogs, count
            ));
            style_row[0] = Cell::new("⚠️");
        }

        table.add_row(style_row);
    }

    println!("\nStyle summary:\n{}", table);

    if !warnings.is_empty() {
        println!("\nWarnings:");
        for warning in warnings {
            println!("{}", warning);
        }
        println!();
    }
}
