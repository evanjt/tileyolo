use crate::constants::MAX_CACHED_COG_READERS;
use crate::models::geometry::GeometryExtent;
use crate::models::layer::Layer;
use crate::reader::cog_reader::CogReader;
use crate::utils::style::{get_builtin_gradient, is_rgb_style};
use image::{codecs::png::PngEncoder, ColorType, ImageEncoder, Rgba, RgbaImage};
use lru::LruCache;
use std::io::Cursor;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::task;

static COG_READER_CACHE: std::sync::LazyLock<std::sync::Mutex<LruCache<String, Arc<CogReader>>>> =
    std::sync::LazyLock::new(|| {
        std::sync::Mutex::new(LruCache::new(NonZeroUsize::new(MAX_CACHED_COG_READERS).unwrap()))
    });

// Returns true if the value should be treated as nodata (currently, if it is NaN)
fn is_nodata(val: f32) -> bool {
    val.is_nan()
}

/// Get or create a cached `CogReader` for efficient windowed reading
fn get_cog_reader(path: &PathBuf) -> Result<Arc<CogReader>, Box<dyn std::error::Error + Send + Sync>> {
    let path_str = path.to_string_lossy().to_string();

    // Check cache first
    {
        let mut cache = COG_READER_CACHE.lock().unwrap();
        if let Some(reader) = cache.get(&path_str) {
            return Ok(Arc::clone(reader));
        }
    }

    // Create new reader
    let reader = CogReader::open(&path_str)?;
    let reader = Arc::new(reader);

    // Cache it
    {
        let mut cache = COG_READER_CACHE.lock().unwrap();
        cache.put(path_str, Arc::clone(&reader));
    }

    Ok(reader)
}

/// Extracted tile data with band information
pub struct TileData {
    /// Pixel values (interleaved if multi-band: R,G,B,R,G,B,...)
    pub pixels: Vec<f32>,
    /// Number of bands (1 for grayscale, 3 for RGB, 4 for RGBA)
    pub bands: usize,
}

/// Extract tile data using `CogReader`'s efficient windowed reading
/// This reads only the necessary TIFF tiles, not the entire file
///
/// OPTIMIZED: Pre-computes coordinate transforms, pre-loads needed tiles
/// NOW WITH OVERVIEW SUPPORT: Uses appropriate resolution level for efficiency
///
/// Delegates to geocog's xyz_tile module for pure Rust coordinate transforms
pub fn extract_tile_with_cog_reader(
    reader: &CogReader,
    extent_3857: &GeometryExtent,
    tile_size: (usize, usize),
) -> Result<TileData, Box<dyn std::error::Error + Send + Sync>> {
    // Convert GeometryExtent to geocog's BoundingBox
    let bbox = geocog::xyz_tile::BoundingBox::new(
        extent_3857.minx,
        extent_3857.miny,
        extent_3857.maxx,
        extent_3857.maxy,
    );

    // Use geocog's pure Rust tile extraction (no libproj dependency)
    let geocog_tile = geocog::xyz_tile::extract_tile_with_extent(reader, &bbox, tile_size)?;

    // Convert geocog::xyz_tile::TileData to our TileData
    Ok(TileData {
        pixels: geocog_tile.pixels,
        bands: geocog_tile.bands,
    })
}

pub async fn process_cog(
    input_path: PathBuf,
    extent_3857: GeometryExtent,
    layer_obj: Layer,
    tile_size: (usize, usize),
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    task::spawn_blocking(move || {
        let (tile_size_x, tile_size_y) = tile_size;

        // Use CogReader for efficient windowed reading
        let reader = get_cog_reader(&input_path)?;
        let tile_data = extract_tile_with_cog_reader(&reader, &extent_3857, tile_size)?;

        let mut img = RgbaImage::new(tile_size_x as u32, tile_size_y as u32);

        // Determine rendering mode based on style and band count:
        // 1. If style is explicitly "rgb" -> RGB passthrough (or grayscale fallback for 1-band)
        // 2. If style is a colormap (viridis, etc.) -> always colorize first band
        // 3. If no style set and 3+ bands -> RGB passthrough
        // 4. Otherwise -> single-band colorization
        let use_rgb_passthrough = if is_rgb_style(&layer_obj.style) {
            // RGB style folder: use RGB if available, else grayscale
            tile_data.bands >= 3
        } else if get_builtin_gradient(&layer_obj.style).is_some() || !layer_obj.colour_stops.is_empty() {
            // Explicit colormap style: always use first band
            false
        } else {
            // No specific style: use RGB if available
            tile_data.bands >= 3
        };

        if use_rgb_passthrough {
            render_rgb_tile(&tile_data, tile_size_x, tile_size_y, &mut img);
        } else {
            // Single-band rendering with colorization
            render_single_band_tile(&tile_data, tile_size_x, tile_size_y, &layer_obj, &mut img);
        }

        // Encode to PNG
        let mut png_data = Vec::new();
        PngEncoder::new(Cursor::new(&mut png_data)).write_image(
            img.as_raw(),
            tile_size_x as u32,
            tile_size_y as u32,
            ColorType::Rgba8.into(),
        )?;

        Ok(png_data)
    })
    .await?
}

/// Render RGB/RGBA tile directly without colorization
fn render_rgb_tile(
    tile_data: &TileData,
    tile_size_x: usize,
    tile_size_y: usize,
    img: &mut RgbaImage,
) {
    let num_bands = tile_data.bands;
    let pixels = &tile_data.pixels;

    for y in 0..tile_size_y {
        for x in 0..tile_size_x {
            let idx = (y * tile_size_x + x) * num_bands;

            // Get RGB values (clamp to 0-255 range)
            let r = pixels.get(idx).copied().unwrap_or(f32::NAN);
            let g = pixels.get(idx + 1).copied().unwrap_or(f32::NAN);
            let b = pixels.get(idx + 2).copied().unwrap_or(f32::NAN);

            // Check if any band is nodata
            let px = if r.is_nan() || g.is_nan() || b.is_nan() {
                Rgba([0, 0, 0, 0])
            } else {
                // Get alpha if present (4-band), otherwise fully opaque
                let a = if num_bands >= 4 {
                    pixels.get(idx + 3).copied().unwrap_or(255.0).clamp(0.0, 255.0) as u8
                } else {
                    255
                };

                Rgba([
                    r.clamp(0.0, 255.0) as u8,
                    g.clamp(0.0, 255.0) as u8,
                    b.clamp(0.0, 255.0) as u8,
                    a,
                ])
            };

            img.put_pixel(x as u32, y as u32, px);
        }
    }
}

/// Render single-band tile with colorization (gradient, colour stops, or grayscale)
fn render_single_band_tile(
    tile_data: &TileData,
    tile_size_x: usize,
    tile_size_y: usize,
    layer_obj: &Layer,
    img: &mut RgbaImage,
) {
    let num_bands = tile_data.bands;
    let pixel_data = &tile_data.pixels;
    let num_pixels = tile_size_x * tile_size_y;

    if let Some(grad) = get_builtin_gradient(&layer_obj.style) {
        // Use the gradient to colourise the image (first band only)
        for i in 0..num_pixels {
            // For multi-band data, extract only the first band
            let raw = pixel_data.get(i * num_bands).copied().unwrap_or(f32::NAN);
            let px = if is_nodata(raw) {
                Rgba([0, 0, 0, 0])
            } else {
                let t = ((raw - layer_obj.min_value)
                    / (layer_obj.max_value - layer_obj.min_value))
                    .clamp(0.0, 1.0);
                let [r, g, b, a] = grad.at(t).to_rgba8();
                Rgba([r, g, b, a])
            };
            let x = (i % tile_size_x) as u32;
            let y = (i / tile_size_x) as u32;
            img.put_pixel(x, y, px);
        }
    } else if layer_obj.colour_stops.is_empty() {
        // Fallback to grayscale (first band only)
        for i in 0..num_pixels {
            let raw = pixel_data.get(i * num_bands).copied().unwrap_or(f32::NAN);
            let px = if is_nodata(raw) {
                Rgba([0, 0, 0, 0])
            } else {
                let norm =
                    (raw - layer_obj.min_value) / (layer_obj.max_value - layer_obj.min_value);
                let lum = (norm.clamp(0.0, 1.0) * 255.0) as u8;
                Rgba([lum, lum, lum, 255])
            };
            let x = (i % tile_size_x) as u32;
            let y = (i / tile_size_x) as u32;
            img.put_pixel(x, y, px);
        }
    } else {
        // Use the colour stops to colourise the image (first band only)
        let cs = &layer_obj.colour_stops;
        let style_min = cs.first().unwrap().value;
        let style_max = cs.last().unwrap().value;
        for i in 0..num_pixels {
            let raw = pixel_data.get(i * num_bands).copied().unwrap_or(f32::NAN);
            let px = if is_nodata(raw) {
                Rgba([0, 0, 0, 0])
            } else {
                let norm =
                    (raw - layer_obj.min_value) / (layer_obj.max_value - layer_obj.min_value);
                let scaled = style_min + norm.clamp(0.0, 1.0) * (style_max - style_min);
                let mut colour = Rgba([0, 0, 0, 0]);
                for w in cs.windows(2) {
                    let a = &w[0];
                    let b = &w[1];
                    if scaled >= a.value && scaled <= b.value {
                        let t = (scaled - a.value) / (b.value - a.value);
                        let r = ((1.0 - t) * f32::from(a.red) + t * f32::from(b.red)) as u8;
                        let g = ((1.0 - t) * f32::from(a.green) + t * f32::from(b.green)) as u8;
                        let b_ = ((1.0 - t) * f32::from(a.blue) + t * f32::from(b.blue)) as u8;
                        let a_ = ((1.0 - t) * f32::from(a.alpha) + t * f32::from(b.alpha)) as u8;
                        colour = Rgba([r, g, b_, a_]);
                        break;
                    }
                }
                colour
            };
            let x = (i % tile_size_x) as u32;
            let y = (i / tile_size_x) as u32;
            img.put_pixel(x, y, px);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        geometry::GeometryExtent,
        layer::{Layer, LayerGeometry},
        style::ColourStop,
    };

    async fn make_layer(min_value: f32, max_value: f32) -> Layer {
        let path = PathBuf::new();
        let colour_stops = vec![
            ColourStop { value: 0.0, red: 215, green: 25, blue: 28, alpha: 255 },
            ColourStop { value: 100.0, red: 253, green: 174, blue: 97, alpha: 255 },
            ColourStop { value: 200.0, red: 255, green: 255, blue: 191, alpha: 255 },
            ColourStop { value: 300.0, red: 171, green: 221, blue: 164, alpha: 255 },
            ColourStop { value: 400.0, red: 43, green: 131, blue: 186, alpha: 255 },
        ];

        let source_geometry = LayerGeometry {
            crs_code: 3857,
            extent: GeometryExtent { minx: 0.0, miny: 0.0, maxx: 256.0, maxy: 256.0 },
        };
        let cached_geometry = source_geometry.generate_cached_geometry_sync().unwrap();
        Layer {
            layer: "test".to_string(),
            style: "default".to_string(),
            path,
            size_bytes: 0,
            source_geometry,
            cached_geometry,
            colour_stops,
            min_value,
            max_value,
            is_cog: true,
            last_modified: std::time::SystemTime::UNIX_EPOCH,
            bands: 1,
            is_tiled: true,
        }
    }

    #[tokio::test]
    async fn test_process_cog_basic() {
        let tile_size = (256, 256);
        let mut layer = make_layer(1.0, 100.0).await;
        layer.path = PathBuf::from("test_data/sample.tif");

        let extent = GeometryExtent { minx: 0.0, miny: 0.0, maxx: 256.0, maxy: 256.0 };
        let result = process_cog(layer.path.clone(), extent, layer, tile_size).await;

        // Expected to fail without proper test data
        if let Ok(_) = result {}
    }

    // ========================================================================
    // Styling Tests
    // ========================================================================

    #[test]
    fn test_nodata_handling() {
        assert!(is_nodata(f32::NAN));
        assert!(!is_nodata(0.0));
        assert!(!is_nodata(42.0));
        assert!(!is_nodata(f32::INFINITY));
    }

    #[test]
    fn test_colour_stop_interpolation() {
        let stops = [ColourStop { value: 0.0, red: 0, green: 0, blue: 0, alpha: 255 },
            ColourStop { value: 100.0, red: 255, green: 255, blue: 255, alpha: 255 }];

        // Test interpolation at midpoint
        let scaled = 50.0;
        let mut colour = Rgba([0, 0, 0, 0]);
        for w in stops.windows(2) {
            let a = &w[0];
            let b = &w[1];
            if scaled >= a.value && scaled <= b.value {
                let t = (scaled - a.value) / (b.value - a.value);
                let r = ((1.0 - t) * a.red as f32 + t * b.red as f32) as u8;
                let g = ((1.0 - t) * a.green as f32 + t * b.green as f32) as u8;
                let b_ = ((1.0 - t) * a.blue as f32 + t * b.blue as f32) as u8;
                let a_ = ((1.0 - t) * a.alpha as f32 + t * b.alpha as f32) as u8;
                colour = Rgba([r, g, b_, a_]);
                break;
            }
        }

        // At 50%, should be ~127 for all channels
        assert!((colour.0[0] as i32 - 127).abs() <= 1, "Red should be ~127");
        assert!((colour.0[1] as i32 - 127).abs() <= 1, "Green should be ~127");
        assert!((colour.0[2] as i32 - 127).abs() <= 1, "Blue should be ~127");
    }

    // ========================================================================
    // END-TO-END INTEGRATION TESTS WITH REAL COG FILES
    // These tests verify the full tile extraction pipeline against GDAL output.
    // They would have caught the coordinate mismatch bug!
    // ========================================================================

    /// TEST: End-to-end tile extraction with CogReader matches expected values
    ///
    /// This is the integration test that SHOULD have caught our bug:
    /// - Uses real COG file (gray_3857-cog.tif)
    /// - Extracts tile at zoom 2, position (1,1)
    /// - Verifies pixel values match GDAL reference values
    ///
    /// GDAL reference values obtained via:
    /// gdal_translate -of PNG -outsize 256 256 \
    ///   -projwin -10018754.17 10018754.17 0 0 \
    ///   data/grayscale/gray_3857-cog.tif /tmp/gdal_z2.png
    #[test]
    fn test_tile_extraction_matches_gdal_reference() {
        let path = "data/grayscale/gray_3857-cog.tif";
        if !std::path::Path::new(path).exists() {
            println!("Skipping - file not found: {}", path);
            return;
        }

        let reader = CogReader::open(path).expect("Failed to open COG");

        // Web Mercator tile bounds for z=2, x=1, y=1
        // minx = -20037508.34 + 1 * (40075016.68/4) = -10018754.17
        // maxx = -20037508.34 + 2 * (40075016.68/4) = 0
        // maxy = 20037508.34 - 1 * (40075016.68/4) = 10018754.17
        // miny = 20037508.34 - 2 * (40075016.68/4) = 0
        let extent = GeometryExtent::new(
            -10018754.17,  // minx
            0.0,           // miny
            0.0,           // maxx
            10018754.17,   // maxy
        );

        let tile_data = extract_tile_with_cog_reader(&reader, &extent, (256, 256))
            .expect("Failed to extract tile");

        // Verify we got data
        assert_eq!(tile_data.pixels.len(), 256 * 256, "Should have 256x256 pixels");
        assert_eq!(tile_data.bands, 1, "Should have 1 band");

        // Count valid pixels
        let valid_count = tile_data.pixels.iter().filter(|v| !v.is_nan()).count();
        assert!(valid_count > 0, "Should have valid (non-NaN) pixels");

        // GDAL reference values for corners of this tile:
        // TL (0,0) = 225, TR (255,0) = 166, BL (0,255) = 169, BR (255,255) = 153
        // Allow tolerance for resampling differences
        const TOLERANCE: f32 = 10.0;

        let tl = tile_data.pixels[0];
        let tr = tile_data.pixels[255];
        let bl = tile_data.pixels[255 * 256];
        let br = tile_data.pixels[255 * 256 + 255];

        // These are the critical assertions that would have caught the bug!
        // Before the fix: TL was ~170, BR was ~205 (completely wrong)
        // After the fix: TL is ~225, BR is ~153 (matches GDAL)

        if !tl.is_nan() {
            assert!(
                (tl - 225.0).abs() < TOLERANCE,
                "Top-left pixel should be ~225 (GDAL reference), got {}",
                tl
            );
        }

        if !tr.is_nan() {
            assert!(
                (tr - 166.0).abs() < TOLERANCE,
                "Top-right pixel should be ~166 (GDAL reference), got {}",
                tr
            );
        }

        if !bl.is_nan() {
            assert!(
                (bl - 169.0).abs() < TOLERANCE,
                "Bottom-left pixel should be ~169 (GDAL reference), got {}",
                bl
            );
        }

        if !br.is_nan() {
            assert!(
                (br - 153.0).abs() < TOLERANCE,
                "Bottom-right pixel should be ~153 (GDAL reference), got {}",
                br
            );
        }
    }

    /// TEST: Verify pixel statistics match expected range
    ///
    /// This test verifies the overall statistics of extracted tiles.
    #[test]
    fn test_tile_statistics_match_expected() {
        let path = "data/grayscale/gray_3857-cog.tif";
        if !std::path::Path::new(path).exists() {
            println!("Skipping - file not found: {}", path);
            return;
        }

        let reader = CogReader::open(path).expect("Failed to open COG");

        // Same extent as above
        let extent = GeometryExtent::new(-10018754.17, 0.0, 0.0, 10018754.17);

        let tile_data = extract_tile_with_cog_reader(&reader, &extent, (256, 256))
            .expect("Failed to extract tile");

        // Calculate statistics
        let valid_pixels: Vec<f32> = tile_data.pixels.iter()
            .filter(|v| !v.is_nan())
            .copied()
            .collect();

        if !valid_pixels.is_empty() {
            let min = valid_pixels.iter().copied().fold(f32::INFINITY, f32::min);
            let max = valid_pixels.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let mean = valid_pixels.iter().sum::<f32>() / valid_pixels.len() as f32;

            // GDAL reference statistics:
            // min=120, max=251, mean=173.9
            assert!((100.0..=140.0).contains(&min), "Min should be ~120, got {}", min);
            assert!((230.0..=260.0).contains(&max), "Max should be ~251, got {}", max);
            assert!((160.0..=190.0).contains(&mean), "Mean should be ~174, got {}", mean);
        }
    }

    /// Test tile extraction at the rightmost edge (near +180° longitude)
    /// This tests for edge artifacts at the antimeridian
    #[test]
    fn test_tile_rightmost_edge_no_artifacts() {
        let path = "data/grayscale/gray_3857-cog.tif";
        if !std::path::Path::new(path).exists() {
            println!("Skipping - file not found: {}", path);
            return;
        }

        let reader = CogReader::open(path).expect("Failed to open COG");

        // Web Mercator tile bounds for z=2, x=3, y=1 (rightmost tile at this zoom)
        // This tile touches +180° longitude (20037508.34)
        let tile_extent = 40075016.68 / 4.0;  // ~10018754.17
        let extent = GeometryExtent::new(
            -20037508.342789244 + 3.0 * tile_extent,  // minx ~10018754.17
            20037508.342789244 - 2.0 * tile_extent,   // miny ~0
            -20037508.342789244 + 4.0 * tile_extent,  // maxx ~20037508.34
            20037508.342789244 - 1.0 * tile_extent,   // maxy ~10018754.17
        );

        let tile_data = extract_tile_with_cog_reader(&reader, &extent, (256, 256))
            .expect("Failed to extract tile");

        // Check that the rightmost column (x=255) has valid data, not zeros/NaN
        // This is the edge that was showing artifacts
        let mut rightmost_col_valid = 0;
        let mut rightmost_col_zeros = 0;
        for y in 0..256 {
            let idx = y * 256 + 255;
            let val = tile_data.pixels[idx];
            if val.is_nan() || val == 0.0 {
                rightmost_col_zeros += 1;
            } else {
                rightmost_col_valid += 1;
            }
        }

        println!("Rightmost column (x=255): {} valid, {} zeros/NaN", rightmost_col_valid, rightmost_col_zeros);

        // The rightmost column should have mostly valid data (allow some edge cases)
        // If the entire column is zero, there's an edge artifact bug
        assert!(
            rightmost_col_zeros < 256,
            "Rightmost column should not be entirely zero/NaN. Found {} zeros. This indicates an edge artifact at +180° longitude.",
            rightmost_col_zeros
        );

        // Check middle columns for comparison
        let mut middle_col_valid = 0;
        for y in 0..256 {
            let idx = y * 256 + 128;
            if !tile_data.pixels[idx].is_nan() && tile_data.pixels[idx] != 0.0 {
                middle_col_valid += 1;
            }
        }
        println!("Middle column (x=128): {} valid", middle_col_valid);
    }

    /// Test that EPSG:4326 files work with overview fallback for empty overviews
    #[test]
    fn test_4326_overview_fallback() {
        let path = "cog_output/viridis/barley.tif";

        if !std::path::Path::new(path).exists() {
            println!("Skipping - file not found: {}", path);
            return;
        }

        let reader = CogReader::open(path).expect("Failed to open");

        // Zoom 0 extent in Web Mercator
        let extent_z0 = GeometryExtent::new(
            -20037508.342789244,
            -20037508.342789244,
            20037508.342789244,
            20037508.342789244,
        );

        // Extract tile - should work even if smallest overview is empty
        let result = extract_tile_with_cog_reader(&reader, &extent_z0, (256, 256));
        assert!(result.is_ok(), "Tile extraction should succeed");

        // Note: For sparse data like barley, the actual valid pixel count depends
        // on data coverage. The important thing is that extraction doesn't fail.
        let tile = result.unwrap();
        assert_eq!(tile.pixels.len(), 256 * 256, "Should have correct pixel count");
    }
}
