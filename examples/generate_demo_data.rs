//! Generate Demo Data and Start Server
//!
//! This example generates synthetic GeoTIFF files with realistic terrain-like patterns
//! and automatically starts the tile server to view them.
//!
//! Run with: cargo run --example generate_demo_data
//!
//! The server will start at http://localhost:8080 and the generated data will be
//! automatically cleaned up when the server is stopped (Ctrl+C).
//!
//! Generated layers:
//! - terrain_highres (viridis) - High-res DEM-like elevation
//! - terrain_medres (turbo) - Medium resolution terrain
//! - terrain_lowres (magma) - Low resolution overview
//! - bathymetry (plasma) - Ocean depth patterns
//! - temperature (inferno) - Temperature gradient
//! - noise_pattern (spectral) - Simple Perlin noise
//! - landcover (rainbow) - Discrete land cover classes

use noise::{NoiseFn, Perlin, Fbm, MultiFractal};
use rayon::prelude::*;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Web Mercator bounds (EPSG:3857)
const WEB_MERCATOR_MIN: f64 = -20037508.342789244;
const WEB_MERCATOR_MAX: f64 = 20037508.342789244;

/// Configuration for a synthetic terrain layer
struct TerrainConfig {
    name: &'static str,
    style_name: &'static str,
    width: usize,
    height: usize,
    min_value: f32,
    max_value: f32,
    generator: TerrainGenerator,
}

#[derive(Clone)]
#[allow(dead_code)]
enum TerrainGenerator {
    Fbm { octaves: usize, frequency: f64, lacunarity: f64, persistence: f64 },
    Perlin { frequency: f64 },
    EarthLike { sea_level: f32 },
    TemperatureGradient,
    LandCover { num_classes: u8 },
}

/// Guard that cleans up the demo data directory when dropped
struct CleanupGuard {
    path: PathBuf,
}

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        println!("\nCleaning up demo data...");
        if let Err(e) = fs::remove_dir_all(&self.path) {
            eprintln!("Warning: Failed to clean up {}: {}", self.path.display(), e);
        } else {
            println!("Demo data removed.");
        }
    }
}

#[tokio::main]
async fn main() {
    println!("=== TileYolo Demo Data Generator ===\n");

    let base_path = PathBuf::from("data/demo");
    let start = Instant::now();

    // Clean up any existing demo data first
    if base_path.exists() {
        println!("Removing existing demo data...");
        let _ = fs::remove_dir_all(&base_path);
    }

    // Define terrain types
    let configs = vec![
        TerrainConfig {
            name: "terrain_highres",
            style_name: "viridis",
            width: 2048,
            height: 2048,
            min_value: -500.0,
            max_value: 4500.0,
            generator: TerrainGenerator::EarthLike { sea_level: 0.0 },
        },
        TerrainConfig {
            name: "terrain_medres",
            style_name: "turbo",
            width: 1024,
            height: 1024,
            min_value: -500.0,
            max_value: 4500.0,
            generator: TerrainGenerator::EarthLike { sea_level: 0.0 },
        },
        TerrainConfig {
            name: "terrain_lowres",
            style_name: "magma",
            width: 512,
            height: 512,
            min_value: -500.0,
            max_value: 4500.0,
            generator: TerrainGenerator::EarthLike { sea_level: 0.0 },
        },
        TerrainConfig {
            name: "bathymetry",
            style_name: "plasma",
            width: 1024,
            height: 1024,
            min_value: -11000.0,
            max_value: 0.0,
            generator: TerrainGenerator::Fbm {
                octaves: 6,
                frequency: 3.0,
                lacunarity: 2.0,
                persistence: 0.5,
            },
        },
        TerrainConfig {
            name: "temperature",
            style_name: "inferno",
            width: 1024,
            height: 512,
            min_value: -40.0,
            max_value: 45.0,
            generator: TerrainGenerator::TemperatureGradient,
        },
        TerrainConfig {
            name: "noise_pattern",
            style_name: "spectral",
            width: 512,
            height: 512,
            min_value: 0.0,
            max_value: 100.0,
            generator: TerrainGenerator::Perlin { frequency: 8.0 },
        },
        TerrainConfig {
            name: "landcover",
            style_name: "rainbow",
            width: 512,
            height: 512,
            min_value: 0.0,
            max_value: 10.0,
            generator: TerrainGenerator::LandCover { num_classes: 10 },
        },
    ];

    println!("Generating {} terrain layers in parallel...\n", configs.len());

    // Generate each terrain type in parallel
    let results: Vec<_> = configs
        .par_iter()
        .map(|config| {
            let layer_start = Instant::now();
            let result = generate_terrain(config, &base_path);
            let elapsed = layer_start.elapsed();
            (config.name, config.style_name, result, elapsed)
        })
        .collect();

    // Print results
    for (name, style, result, elapsed) in results {
        match result {
            Ok(_) => println!("  {} ({}) - {:.2}s", name, style, elapsed.as_secs_f64()),
            Err(e) => eprintln!("  {} - Error: {}", name, e),
        }
    }

    let gen_elapsed = start.elapsed();
    println!("\nGeneration complete in {:.2}s", gen_elapsed.as_secs_f64());

    // Create cleanup guard - will clean up data when server stops
    let _cleanup = CleanupGuard { path: base_path.clone() };

    // Start the tile server
    println!("\n=== Starting Tile Server ===");
    println!("Server will be available at: http://localhost:8080");
    println!("Press Ctrl+C to stop the server and clean up demo data.\n");

    // Use the library to start the server
    let config = tileyolo::Config {
        source: Some(tileyolo::Source::Local(base_path)),
        data_folder: "data/demo".to_string(),
        port: 8080,
        ..Default::default()
    };

    let server = match tileyolo::TileServer::new(config).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to create server: {}", e);
            return;
        }
    };

    // Run until interrupted
    if let Err(e) = server.start().await {
        eprintln!("Server error: {}", e);
    }

    // CleanupGuard will automatically clean up when we exit
}

fn generate_terrain(config: &TerrainConfig, base_path: &Path) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let style_dir = base_path.join(config.style_name);
    fs::create_dir_all(&style_dir)?;

    let data = generate_raster_data(config);

    let tiff_path = style_dir.join(format!("{}.tif", config.name));
    write_geotiff(&tiff_path, &data, config.width, config.height)?;

    Ok(())
}

fn generate_raster_data(config: &TerrainConfig) -> Vec<f32> {
    let width = config.width;
    let height = config.height;
    let min_val = config.min_value;
    let max_val = config.max_value;
    let generator = config.generator.clone();

    let rows: Vec<Vec<f32>> = (0..height)
        .into_par_iter()
        .map(|y| generate_row(y, width, height, min_val, max_val, &generator))
        .collect();

    rows.into_iter().flatten().collect()
}

fn generate_row(
    y: usize,
    width: usize,
    height: usize,
    min_val: f32,
    max_val: f32,
    generator: &TerrainGenerator,
) -> Vec<f32> {
    let mut row = vec![0.0f32; width];
    let perlin = Perlin::new(42);

    match generator {
        TerrainGenerator::Fbm { octaves, frequency, lacunarity, persistence } => {
            let fbm: Fbm<Perlin> = Fbm::new(42)
                .set_octaves(*octaves)
                .set_frequency(*frequency)
                .set_lacunarity(*lacunarity)
                .set_persistence(*persistence);

            for x in 0..width {
                let nx = x as f64 / width as f64;
                let ny = y as f64 / height as f64;
                let noise_val = fbm.get([nx, ny]);
                let normalized = (noise_val + 1.0) / 2.0;
                row[x] = min_val + (normalized as f32) * (max_val - min_val);
            }
        }

        TerrainGenerator::Perlin { frequency } => {
            for x in 0..width {
                let nx = x as f64 / width as f64 * frequency;
                let ny = y as f64 / height as f64 * frequency;
                let noise_val = perlin.get([nx, ny]);
                let normalized = (noise_val + 1.0) / 2.0;
                row[x] = min_val + (normalized as f32) * (max_val - min_val);
            }
        }

        TerrainGenerator::EarthLike { sea_level: _ } => {
            let continent_noise: Fbm<Perlin> = Fbm::new(42)
                .set_octaves(4)
                .set_frequency(2.0)
                .set_persistence(0.5);

            let detail_noise: Fbm<Perlin> = Fbm::new(123)
                .set_octaves(6)
                .set_frequency(8.0)
                .set_persistence(0.4);

            let mountain_noise: Fbm<Perlin> = Fbm::new(456)
                .set_octaves(4)
                .set_frequency(4.0)
                .set_persistence(0.6);

            for x in 0..width {
                let nx = x as f64 / width as f64;
                let ny = y as f64 / height as f64;

                let continent = continent_noise.get([nx, ny]);
                let is_land = continent > -0.1;

                let elevation = if is_land {
                    let base = (continent + 1.0) / 2.0 * 1000.0;
                    let detail = detail_noise.get([nx, ny]) * 200.0;
                    let mountain_factor = (continent - 0.3).max(0.0) * 2.0;
                    let mountains = mountain_noise.get([nx, ny]).max(0.0) * mountain_factor * 3000.0;
                    base + detail + mountains
                } else {
                    let depth_factor = (-continent - 0.1).max(0.0);
                    let base_depth = depth_factor * 4000.0;
                    let detail = detail_noise.get([nx, ny]) * 100.0;
                    -(base_depth + detail.abs())
                };

                row[x] = elevation.clamp(min_val as f64, max_val as f64) as f32;
            }
        }

        TerrainGenerator::TemperatureGradient => {
            let weather_noise: Fbm<Perlin> = Fbm::new(789)
                .set_octaves(4)
                .set_frequency(6.0)
                .set_persistence(0.4);

            for x in 0..width {
                let nx = x as f64 / width as f64;
                let ny = y as f64 / height as f64;

                let latitude_factor = 1.0 - 2.0 * (ny - 0.5).abs();
                let base_temp = min_val as f64 + latitude_factor * (max_val - min_val) as f64;
                let weather = weather_noise.get([nx, ny]) * 10.0;

                row[x] = (base_temp + weather).clamp(min_val as f64, max_val as f64) as f32;
            }
        }

        TerrainGenerator::LandCover { num_classes } => {
            let class_noise: Fbm<Perlin> = Fbm::new(999)
                .set_octaves(3)
                .set_frequency(4.0)
                .set_persistence(0.5);

            for x in 0..width {
                let nx = x as f64 / width as f64;
                let ny = y as f64 / height as f64;

                let noise_val = class_noise.get([nx, ny]);
                let normalized = ((noise_val + 1.0) / 2.0).clamp(0.0, 0.999);
                let class_val = (normalized * *num_classes as f64).floor();

                row[x] = class_val as f32;
            }
        }
    }

    row
}

fn write_geotiff(
    path: &Path,
    data: &[f32],
    width: usize,
    height: usize,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);

    let mut tiff_data = Vec::new();

    // TIFF Header
    tiff_data.extend_from_slice(&[0x49, 0x49]); // Little-endian
    tiff_data.extend_from_slice(&42u16.to_le_bytes());
    tiff_data.extend_from_slice(&8u32.to_le_bytes());

    let ifd_offset = 8u32;
    let num_tags = 16u16;
    let ifd_size = 2 + (num_tags as usize * 12) + 4;
    let extra_values_offset = ifd_offset as usize + ifd_size;

    let pixel_scale = [
        (WEB_MERCATOR_MAX - WEB_MERCATOR_MIN) / width as f64,
        (WEB_MERCATOR_MAX - WEB_MERCATOR_MIN) / height as f64,
        0.0,
    ];
    let tiepoint = [0.0, 0.0, 0.0, WEB_MERCATOR_MIN, WEB_MERCATOR_MAX, 0.0];
    let geo_key_directory: [u16; 16] = [
        1, 1, 0, 3,
        1024, 0, 1, 1,
        1025, 0, 1, 1,
        3072, 0, 1, 3857,
    ];

    let pixel_scale_offset = extra_values_offset;
    let tiepoint_offset = pixel_scale_offset + 24;
    let geo_key_offset = tiepoint_offset + 48;
    let image_data_offset = geo_key_offset + 32;
    let image_bytes = data.len() * 4;

    let mut ifd = Vec::new();
    ifd.extend_from_slice(&num_tags.to_le_bytes());

    fn write_tag(ifd: &mut Vec<u8>, tag: u16, typ: u16, count: u32, value: u32) {
        ifd.extend_from_slice(&tag.to_le_bytes());
        ifd.extend_from_slice(&typ.to_le_bytes());
        ifd.extend_from_slice(&count.to_le_bytes());
        ifd.extend_from_slice(&value.to_le_bytes());
    }

    write_tag(&mut ifd, 256, 3, 1, width as u32);
    write_tag(&mut ifd, 257, 3, 1, height as u32);
    write_tag(&mut ifd, 258, 3, 1, 32);
    write_tag(&mut ifd, 259, 3, 1, 1);
    write_tag(&mut ifd, 262, 3, 1, 1);
    write_tag(&mut ifd, 273, 4, 1, image_data_offset as u32);
    write_tag(&mut ifd, 277, 3, 1, 1);
    write_tag(&mut ifd, 278, 3, 1, height as u32);
    write_tag(&mut ifd, 279, 4, 1, image_bytes as u32);
    write_tag(&mut ifd, 284, 3, 1, 1);
    write_tag(&mut ifd, 339, 3, 1, 3);
    write_tag(&mut ifd, 33550, 12, 3, pixel_scale_offset as u32);
    write_tag(&mut ifd, 33922, 12, 6, tiepoint_offset as u32);
    write_tag(&mut ifd, 34735, 3, 16, geo_key_offset as u32);
    write_tag(&mut ifd, 42113, 2, 4, 0x4e614e00);
    write_tag(&mut ifd, 0, 0, 0, 0);

    ifd.extend_from_slice(&0u32.to_le_bytes());

    tiff_data.extend_from_slice(&ifd);

    for val in &pixel_scale {
        tiff_data.extend_from_slice(&val.to_le_bytes());
    }
    for val in &tiepoint {
        tiff_data.extend_from_slice(&val.to_le_bytes());
    }
    for val in &geo_key_directory {
        tiff_data.extend_from_slice(&val.to_le_bytes());
    }
    for val in data {
        tiff_data.extend_from_slice(&val.to_le_bytes());
    }

    writer.write_all(&tiff_data)?;
    writer.flush()?;

    Ok(())
}
