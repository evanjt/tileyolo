//! CLI tool to generate deterministic test COG files.
//!
//! Usage:
//!   cargo run --bin generate_test_cog -- output.tif [--size 512] [--pattern position]

use std::env;
use std::path::PathBuf;
use tileyolo::test_cog_generator::{TestCogGenerator, TestPattern};

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: {} <output.tif> [--size N] [--pattern TYPE]", args[0]);
        eprintln!();
        eprintln!("Patterns: position, checkerboard, horizontal, vertical, diagonal, rings, solid");
        eprintln!();
        eprintln!("Example:");
        eprintln!("  {} test.tif --size 512 --pattern position", args[0]);
        std::process::exit(1);
    }

    let output_path = PathBuf::from(&args[1]);
    let mut size = 512u32;
    let mut pattern = TestPattern::PositionEncoded;

    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--size" => {
                i += 1;
                if i < args.len() {
                    size = args[i].parse().expect("Invalid size");
                }
            }
            "--pattern" => {
                i += 1;
                if i < args.len() {
                    pattern = match args[i].as_str() {
                        "position" => TestPattern::PositionEncoded,
                        "checkerboard" => TestPattern::Checkerboard { cell_size: 32 },
                        "horizontal" => TestPattern::HorizontalGradient,
                        "vertical" => TestPattern::VerticalGradient,
                        "diagonal" => TestPattern::DiagonalGradient,
                        "rings" => TestPattern::ConcentricRings { ring_width: 20 },
                        "solid" => TestPattern::Solid { value: 128 },
                        other => {
                            eprintln!("Unknown pattern: {}", other);
                            std::process::exit(1);
                        }
                    };
                }
            }
            _ => {}
        }
        i += 1;
    }

    let generator = TestCogGenerator::new()
        .size(size, size)
        .world_bounds()
        .pattern(pattern);

    println!("Generating {}x{} test COG with {:?} pattern...", size, size, pattern);

    match generator.write_tiff(&output_path) {
        Ok(()) => {
            println!("Written to: {}", output_path.display());
            println!();
            println!("Ground truth values:");
            println!("  Pixel (0, 0): {}", generator.expected_value(0, 0));
            println!("  Pixel ({}, 0): {}", size / 2, generator.expected_value(size / 2, 0));
            println!("  Pixel (0, {}): {}", size / 2, generator.expected_value(0, size / 2));
            println!(
                "  Pixel ({}, {}): {}",
                size / 2,
                size / 2,
                generator.expected_value(size / 2, size / 2)
            );
        }
        Err(e) => {
            eprintln!("Error writing TIFF: {}", e);
            std::process::exit(1);
        }
    }
}
