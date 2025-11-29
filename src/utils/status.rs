use crate::{
    models::{layer::Layer, style::ColourStop},
    utils::style::{get_builtin_gradient, is_builtin_palette, is_rgb_style},
};
use comfy_table::{Attribute, Cell, CellAlignment, Table};
use std::collections::HashMap;

/// Style info: (layer_count, colour_stops, min_value, max_value, cog_count, rgb_count, non_rgb_count)
type StyleInfo = (usize, Vec<ColourStop>, f32, f32, usize, usize, usize);

pub fn print_layer_summary(layers: &Vec<Layer>) {
    let mut style_info: HashMap<String, StyleInfo> = HashMap::new();
    for layer in layers {
        let entry = style_info.entry(layer.style.clone()).or_insert((
            0,
            layer.colour_stops.clone(),
            layer.min_value,
            layer.max_value,
            0,
            0, // rgb_count
            0, // non_rgb_count
        ));
        entry.0 += 1;
        entry.1 = layer.colour_stops.clone();
        entry.2 = entry.2.min(layer.min_value);
        entry.3 = entry.3.max(layer.max_value);
        entry.4 += layer.is_cog as usize;
        // Track RGB vs non-RGB layers
        if layer.bands >= 3 {
            entry.5 += 1; // rgb_count
        } else {
            entry.6 += 1; // non_rgb_count
        }
    }

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
    let mut cog_error_count: usize = 0;
    for (style, (count, stops, min_v, max_v, num_cogs, _rgb_count, non_rgb_count)) in style_info {
        let is_rgb_folder = is_rgb_style(&style);

        let breaks_str = if is_rgb_folder {
            "RGB".to_string()
        } else if is_builtin_palette(&style) || stops.is_empty() {
            "auto".to_string()
        } else {
            stops
                .iter()
                .map(|s| format!("{:.2}", s.value))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let bar = if is_rgb_folder {
            // RGB colorbar representation
            "\x1b[38;2;255;0;0m█\x1b[0m\x1b[38;2;255;128;0m█\x1b[0m\x1b[38;2;255;255;0m█\x1b[0m\x1b[38;2;0;255;0m█\x1b[0m\x1b[38;2;0;255;255m█\x1b[0m\x1b[38;2;0;128;255m█\x1b[0m\x1b[38;2;0;0;255m█\x1b[0m\x1b[38;2;128;0;255m█\x1b[0m\x1b[38;2;255;0;255m█\x1b[0m\x1b[38;2;255;0;128m█\x1b[0m".to_string()
        } else if let Some(grad) = get_builtin_gradient(&style) {
            let mut s = String::new();
            let n = 10;
            for i in 0..n {
                let t = i as f32 / (n - 1) as f32;
                let [r, g, b, _] = grad.at(t).to_rgba8();
                s.push_str(&format!("\x1b[38;2;{};{};{}m█\x1b[0m", r, g, b));
            }
            s
        } else if stops.is_empty() {
            // fallback to grayscale gradient
            let mut s = String::new();
            let n = 10;
            for i in 0..n {
                let v = (255.0 * i as f32 / (n - 1) as f32).round() as u8;
                s.push_str(&format!("\x1b[38;2;{0};{0};{0}m█\x1b[0m", v));
            }
            s
        } else {
            let mut s = String::new();
            for cs in &stops {
                s.push_str(&format!(
                    "\x1b[38;2;{};{};{}m█\x1b[0m",
                    cs.red, cs.green, cs.blue
                ));
            }
            s
        };

        let style_str = style.clone();
        let mut style_row = vec![
            Cell::new("✅").set_alignment(CellAlignment::Center), // Default success overwritten to warning if needed
            Cell::new(style),
            Cell::new(count).set_alignment(CellAlignment::Center),
            Cell::new(breaks_str).set_alignment(CellAlignment::Center),
            Cell::new(min_v).set_alignment(CellAlignment::Center),
            Cell::new(max_v).set_alignment(CellAlignment::Center),
            Cell::new(bar),
        ];

        if !stops.is_empty() {
            let style_min = stops.first().unwrap().value;
            let style_max = stops.last().unwrap().value;
            if min_v < style_min || max_v > style_max {
                warnings.push(format!(
                    "  ⚠️{}: Colour stops [{:.2}…{:.2}] do NOT cover data range [{:.2}…{:.2}]",
                    style_str, style_min, style_max, min_v, max_v
                ));
                style_row[0] = Cell::new("⚠️");
            }
        }

        if num_cogs < count {
            warnings.push(format!(
                "  ⚠️{}: {} of {} layers are COGs, performance will be degraded on large datasets",
                style_str, num_cogs, count
            ));
            style_row[0] = Cell::new("⚠️");
            cog_error_count += 1;
        }

        // Warn about non-RGB images in RGB folder (will fall back to grayscale)
        if is_rgb_folder && non_rgb_count > 0 {
            warnings.push(format!(
                "  ⚠️{}: {} of {} layers are single-band (will render as grayscale instead of RGB)",
                style_str, non_rgb_count, count
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
    }

    // Use tips section to provide additional information if a warning/error is
    // present. For now we just have COGs... Let's provide a link to some
    // instructions if any are detected
    if cog_error_count > 0 {
        println!("\nTips:");
        println!("  How to generate COGs: https://cogeo.org/developers-guide.html");
    }

    println!();
}
