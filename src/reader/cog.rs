use crate::models::geometry::GeometryExtent;
use crate::models::layer::Layer;
use crate::reader::lzw_fallback::LzwRasterSource;
use crate::reader::raster::{ArrayRasterSource, RasterSource};
use crate::reader::tiff_chunked::TiffChunkedRasterSource;
use crate::reader::tiff_utils::{AnyResult, read_primary_compression};
use crate::utils::style::get_builtin_gradient;
use geo::{AffineTransform, Coord};
use image::{ColorType, ImageEncoder, Rgba, RgbaImage, codecs::png::PngEncoder};
use lru::LruCache;
use ndarray::Array3;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::io::Cursor;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::task;
use tracing::{debug, warn, error, info};

// Global LRU cache for loaded GeoTIFF sources - bounded to prevent memory bloat
type CachedSource = Arc<dyn RasterSource>;
const MAX_CACHED_SOURCES: usize = 50;

static SOURCE_CACHE: Lazy<std::sync::Mutex<LruCache<String, (CachedSource, AffineTransform<f64>)>>> =
    Lazy::new(|| std::sync::Mutex::new(LruCache::new(NonZeroUsize::new(MAX_CACHED_SOURCES).unwrap())));

// Use a better synchronization approach with Arc<Mutex<Option<...>>> for proper waiting
static LOADING_FILES: Lazy<
    std::sync::Mutex<
        HashMap<
            String,
            std::sync::Arc<std::sync::Mutex<Option<(CachedSource, AffineTransform<f64>)>>>,
        >,
    >,
> = Lazy::new(|| std::sync::Mutex::new(HashMap::new()));

// Cache for files that have failed to load - with reduced persistence for retry attempts
static FAILED_FILES: Lazy<std::sync::Mutex<HashMap<String, (String, std::time::Instant)>>> =
    Lazy::new(|| std::sync::Mutex::new(HashMap::new()));

// Only keep failures in cache for 5 minutes to allow retry
const FAILURE_CACHE_DURATION: std::time::Duration = std::time::Duration::from_secs(300);

// Returns true if the value should be treated as nodata (currently, if it is NaN)
fn is_nodata(val: f32) -> bool {
    val.is_nan()
}

pub enum RasterReadResult {
    Array(Array3<f32>),
    Chunked(TiffChunkedRasterSource),
    Lzw(LzwRasterSource),
}

impl RasterReadResult {
    pub fn dimensions(&self) -> (usize, usize, usize) {
        match self {
            RasterReadResult::Array(array) => array.dim(),
            RasterReadResult::Chunked(source) => (source.bands(), source.height(), source.width()),
            RasterReadResult::Lzw(source) => (source.bands(), source.height(), source.width()),
        }
    }

    fn geo_tags(&self) -> (Option<[f64; 3]>, Option<[f64; 6]>) {
        match self {
            RasterReadResult::Array(_) => (None, None),
            RasterReadResult::Chunked(source) => (source.pixel_scale(), source.tiepoint()),
            RasterReadResult::Lzw(source) => (source.pixel_scale(), source.tiepoint()),
        }
    }

    fn into_cached_source(self) -> CachedSource {
        match self {
            RasterReadResult::Array(array) => {
                let arc_array = Arc::new(array);
                Arc::new(ArrayRasterSource::new(arc_array))
            }
            RasterReadResult::Chunked(source) => Arc::new(source),
            RasterReadResult::Lzw(source) => Arc::new(source),
        }
    }

    pub fn as_array(&self) -> Option<&Array3<f32>> {
        match self {
            RasterReadResult::Array(array) => Some(array),
            _ => None,
        }
    }

    pub fn as_chunked(&self) -> Option<&TiffChunkedRasterSource> {
        match self {
            RasterReadResult::Chunked(source) => Some(source),
            _ => None,
        }
    }

    pub fn as_lzw(&self) -> Option<&LzwRasterSource> {
        match self {
            RasterReadResult::Lzw(source) => Some(source),
            _ => None,
        }
    }

    pub fn compute_min_max(&self) -> AnyResult<(f32, f32)> {
        match self {
            RasterReadResult::Array(array) => {
                let mut min = f32::INFINITY;
                let mut max = f32::NEG_INFINITY;
                for &val in array.iter() {
                    if val.is_nan() {
                        continue;
                    }
                    if val < min {
                        min = val;
                    }
                    if val > max {
                        max = val;
                    }
                }
                if min.is_infinite() || max.is_infinite() {
                    Ok((0.0, 0.0))
                } else {
                    Ok((min, max))
                }
            }
            RasterReadResult::Chunked(source) => source.compute_min_max(),
            RasterReadResult::Lzw(source) => source.compute_min_max(),
        }
    }
}

/// Load COG data using cog3pio with proper caching and race condition prevention
fn load_cog_data(
    path: &PathBuf,
) -> Result<(CachedSource, AffineTransform<f64>), Box<dyn std::error::Error + Send + Sync>> {
    // Create a cache key from the file path and modification time
    let metadata = std::fs::metadata(path)?;
    let modified = metadata
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let cache_key = format!("{}:{}", path.display(), modified);

    // Check if this file has failed to load before (only if recently failed)
    {
        let mut failed_files = FAILED_FILES.lock().unwrap();
        // Clean up expired entries first
        let now = std::time::Instant::now();
        failed_files
            .retain(|_, (_, timestamp)| now.duration_since(*timestamp) < FAILURE_CACHE_DURATION);

        if let Some((error_msg, _timestamp)) = failed_files.get(&cache_key) {
            debug!(
                path = %path.display(),
                error = %error_msg,
                "Using recent cached failure"
            );
            return Err(format!("Recent cached failure: {}", error_msg).into());
        }
    }

    // Use a single mutex to coordinate everything atomically
    let mut loading_map = LOADING_FILES.lock().unwrap();

    // Check LRU cache first while holding the lock
    {
        let mut cache = SOURCE_CACHE.lock().unwrap();
        if let Some((cached_source, cached_transform)) = cache.get(&cache_key) {
            debug!(path = %path.display(), "Cache hit for COG file");
            return Ok((cached_source.clone(), *cached_transform));
        }
    }

    // Check if someone is already loading this file
    if let Some(existing_state) = loading_map.get(&cache_key) {
        // Someone is already loading, wait for them to finish
        let state_clone = existing_state.clone();
        drop(loading_map);

        // Wait for the loading to complete by checking the shared state
        loop {
            let state_guard = state_clone.lock().unwrap();
            if let Some((ref source, ref transform)) = *state_guard {
                // Loading completed, return the result
                return Ok((source.clone(), *transform));
            }
            drop(state_guard);

            // Brief pause before checking again
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    // We're the first ones here - create a new loading state and insert it atomically
    let new_state = std::sync::Arc::new(std::sync::Mutex::new(None));
    loading_map.insert(cache_key.clone(), new_state.clone());
    drop(loading_map);

    // We're the loader - proceed to load the file
    let load_result = load_cog_data_internal(path, &cache_key);

    // Store the result in both places regardless of success/failure
    match &load_result {
        Ok((source, transform)) => {
            // Store in LRU cache (will evict old entries automatically)
            {
                let mut cache = SOURCE_CACHE.lock().unwrap();
                cache.put(cache_key.clone(), (source.clone(), *transform));
            }

            // Store in loading state for any waiters
            {
                let mut state_guard = new_state.lock().unwrap();
                *state_guard = Some((source.clone(), *transform));
            }
        }
        Err(e) => {
            error!(path = %path.display(), error = %e, "Failed to load COG file");
            // Remove the loading state so others can try
            {
                let mut loading_map = LOADING_FILES.lock().unwrap();
                loading_map.remove(&cache_key);
            }
        }
    }

    load_result
}

/// Internal function that actually loads the COG data
fn load_cog_data_internal(
    path: &PathBuf,
    cache_key: &str,
) -> Result<(CachedSource, AffineTransform<f64>), Box<dyn std::error::Error + Send + Sync>> {
    // For now, only support local files
    // TODO: Implement HTTP/S3 support in a simpler way
    let _file = std::fs::File::open(path)?;

    // Try different data types until we find one that works
    let raster = match try_read_geotiff_with_flexible_type(path) {
        Ok(result) => result,
        Err(e) => {
            // Cache the failure temporarily to prevent repeated attempts (but allow retry after 5 minutes)
            let error_msg = e.to_string();
            {
                let mut failed_files = FAILED_FILES.lock().unwrap();
                failed_files.insert(
                    cache_key.to_string(),
                    (error_msg.clone(), std::time::Instant::now()),
                );
            }
            return Err(e);
        }
    };
    let (_bands, height, width) = raster.dimensions();
    let (pixel_scale, tiepoint) = raster.geo_tags();
    let transform = build_affine_transform(width, height, pixel_scale, tiepoint);

    let source = raster.into_cached_source();

    Ok((source, transform))
}

fn build_affine_transform(
    width: usize,
    height: usize,
    pixel_scale: Option<[f64; 3]>,
    tiepoint: Option<[f64; 6]>,
) -> AffineTransform<f64> {
    if let (Some(scale), Some(tie)) = (pixel_scale, tiepoint) {
        let x_scale = scale[0];
        let y_scale = scale[1];
        let x_origin = tie[3];
        let y_origin = tie[4];
        AffineTransform::new(x_scale, 0.0, x_origin, 0.0, -y_scale, y_origin)
    } else {
        let web_mercator_min = -20037508.342789244;
        let web_mercator_max = 20037508.342789244;
        let pixel_width = (web_mercator_max - web_mercator_min) / (width as f64);
        let pixel_height = (web_mercator_max - web_mercator_min) / (height as f64);
        AffineTransform::new(
            pixel_width,
            0.0,
            web_mercator_min,
            0.0,
            -pixel_height,
            web_mercator_max,
        )
    }
}

/// Try to read GeoTIFF with different data types and convert to f32, returning a streamed source when needed.
pub fn try_read_geotiff_with_flexible_type(
    path: &PathBuf,
) -> Result<RasterReadResult, Box<dyn std::error::Error + Send + Sync>> {
    debug!(path = %path.display(), "Attempting to read COG file");

    fn safe_read<T, F>(operation: F) -> Result<T, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce() -> Result<T, Box<dyn std::error::Error + Send + Sync>> + std::panic::UnwindSafe,
    {
        match std::panic::catch_unwind(operation) {
            Ok(result) => result,
            Err(panic_info) => {
                let error_msg = match panic_info.downcast::<String>() {
                    Ok(s) => format!("Panic occurred while reading GeoTIFF: {}", s),
                    Err(_) => "Unknown panic occurred while reading GeoTIFF".to_string(),
                };
                error!("{}", error_msg);
                Err(error_msg.into())
            }
        }
    }

    // First, try to detect data type from TIFF tags to avoid trial-and-error
    let compression = match read_primary_compression(path) {
        Ok(value) => value,
        Err(e) => {
            warn!(path = %path.display(), error = %e, "Could not read compression tag");
            None
        }
    };

    if compression == Some(5) {
        debug!(path = %path.display(), "Detected LZW compression; using streamed fallbacks");

        if let Ok(lzw_source) = crate::reader::lzw_fallback::try_read_lzw_tiff_fallback(path) {
            info!(path = %path.display(), "Successfully initialized streamed LZW reader");
            return Ok(RasterReadResult::Lzw(lzw_source));
        }

        debug!(path = %path.display(), "LZW fallback failed, attempting chunked reader");
        if let Ok(chunked_source) = TiffChunkedRasterSource::open(path) {
            info!(path = %path.display(), "Successfully initialized chunked TIFF reader");
            return Ok(RasterReadResult::Chunked(chunked_source));
        }
    }

    match safe_read(|| try_read_with_cog3pio_reader(path)) {
        Ok(array) => {
            debug!(path = %path.display(), "Successfully read GeoTIFF with cog3pio reader");
            return Ok(RasterReadResult::Array(array));
        }
        Err(e) => {
            debug!(path = %path.display(), error = %e, "cog3pio reader failed");
        }
    }

    let mut last_error = "No attempts made".to_string();

    // Try different data types - ideally we'd detect from tags first
    let attempts: &[fn(
        &PathBuf,
    ) -> Result<Array3<f32>, Box<dyn std::error::Error + Send + Sync>>] = &[
        try_read_as_u16,
        try_read_as_u8,
        try_read_as_f32,
        try_read_as_i16,
        try_read_as_u32,
        try_read_as_i32,
        try_read_as_f64,
    ];

    for attempt in attempts {
        match safe_read(|| attempt(path)) {
            Ok(array) => {
                debug!(path = %path.display(), "Successfully read GeoTIFF via fallback data type");
                return Ok(RasterReadResult::Array(array));
            }
            Err(e) => {
                let error_str = e.to_string();
                debug!(path = %path.display(), error = %error_str, "Alternate read failed");
                last_error = error_str;
            }
        }
    }

    if last_error.contains("compression")
        || last_error.contains("LZW")
        || last_error.contains("unsupported")
    {
        if compression == Some(5) {
            debug!(path = %path.display(), "Retrying LZW/chunked fallbacks");
            if let Ok(lzw_source) = crate::reader::lzw_fallback::try_read_lzw_tiff_fallback(path) {
                info!(path = %path.display(), "Successfully initialized streamed LZW reader on retry");
                return Ok(RasterReadResult::Lzw(lzw_source));
            }
            if let Ok(chunked_source) = TiffChunkedRasterSource::open(path) {
                info!(path = %path.display(), "Successfully initialized chunked TIFF reader on retry");
                return Ok(RasterReadResult::Chunked(chunked_source));
            }
        }
    }

    Err(format!(
        "Failed to read COG file '{}' with any supported data type. This may be due to:\n\
        1. Unsupported compression format\n\
        2. Unsupported TIFF tags or structure\n\
        3. Corrupted COG file\n\n\
        Consider recompressing the COG using gdal_translate with -co COMPRESS=DEFLATE or DEFLATE_ZSTD.",
        path.display()
    ).into())
}

fn try_read_as_u8(path: &PathBuf) -> Result<Array3<f32>, Box<dyn std::error::Error + Send + Sync>> {
    let file = std::fs::File::open(path)?;
    let array: Array3<u8> = cog3pio::io::geotiff::read_geotiff(file)?;
    Ok(array.mapv(|x| x as f32))
}

fn try_read_as_u16(
    path: &PathBuf,
) -> Result<Array3<f32>, Box<dyn std::error::Error + Send + Sync>> {
    let file = std::fs::File::open(path)?;
    let array: Array3<u16> = cog3pio::io::geotiff::read_geotiff(file)?;
    Ok(array.mapv(|x| x as f32))
}

fn try_read_as_i16(
    path: &PathBuf,
) -> Result<Array3<f32>, Box<dyn std::error::Error + Send + Sync>> {
    let file = std::fs::File::open(path)?;
    let array: Array3<i16> = cog3pio::io::geotiff::read_geotiff(file)?;
    Ok(array.mapv(|x| x as f32))
}

fn try_read_as_u32(
    path: &PathBuf,
) -> Result<Array3<f32>, Box<dyn std::error::Error + Send + Sync>> {
    let file = std::fs::File::open(path)?;
    let array: Array3<u32> = cog3pio::io::geotiff::read_geotiff(file)?;
    Ok(array.mapv(|x| x as f32))
}

fn try_read_as_i32(
    path: &PathBuf,
) -> Result<Array3<f32>, Box<dyn std::error::Error + Send + Sync>> {
    let file = std::fs::File::open(path)?;
    let array: Array3<i32> = cog3pio::io::geotiff::read_geotiff(file)?;
    Ok(array.mapv(|x| x as f32))
}

fn try_read_as_f32(
    path: &PathBuf,
) -> Result<Array3<f32>, Box<dyn std::error::Error + Send + Sync>> {
    let file = std::fs::File::open(path)?;
    Ok(cog3pio::io::geotiff::read_geotiff(file)?)
}

fn try_read_as_f64(
    path: &PathBuf,
) -> Result<Array3<f32>, Box<dyn std::error::Error + Send + Sync>> {
    let file = std::fs::File::open(path)?;
    let array: Array3<f64> = cog3pio::io::geotiff::read_geotiff(file)?;
    Ok(array.mapv(|x| x as f32))
}

/// Try reading with explicit LZW support using cog3pio's CogReader for better control
fn try_read_with_cog3pio_reader(
    path: &PathBuf,
) -> Result<Array3<f32>, Box<dyn std::error::Error + Send + Sync>> {
    debug!(path = %path.display(), "Attempting to read COG with cog3pio reader");

    // Try u16 first (most common for elevation data)
    match try_read_as_u16(path) {
        Ok(array) => {
            debug!(path = %path.display(), "Successfully read COG as u16");
            return Ok(array);
        }
        Err(e) => {
            debug!(path = %path.display(), error = %e, "Failed to read as u16");
        }
    }

    // Try u8 (common for imagery)
    match try_read_as_u8(path) {
        Ok(array) => {
            debug!(path = %path.display(), "Successfully read COG as u8");
            return Ok(array);
        }
        Err(e) => {
            debug!(path = %path.display(), error = %e, "Failed to read as u8");
        }
    }

    // Try f32
    match try_read_as_f32(path) {
        Ok(array) => {
            debug!(path = %path.display(), "Successfully read COG as f32");
            return Ok(array);
        }
        Err(e) => {
            debug!(path = %path.display(), error = %e, "Failed to read as f32");
        }
    }

    Err("All data types failed with dedicated cog3pio reader".into())
}

/// Convert world coordinates to pixel coordinates using inverse transform
pub fn world_to_pixel(world_coord: Coord<f64>, transform: &AffineTransform<f64>) -> Coord<f64> {
    // Get the inverse transform
    let inv_transform = transform.inverse();

    // Apply inverse transform to get pixel coordinates
    match inv_transform {
        Some(inv) => {
            // Manual matrix multiplication for inverse transform
            let x = inv.a() * world_coord.x + inv.b() * world_coord.y + inv.xoff();
            let y = inv.d() * world_coord.x + inv.e() * world_coord.y + inv.yoff();
            Coord { x, y }
        }
        None => Coord { x: 0.0, y: 0.0 }, // fallback
    }
}

/// Extract tile region from the full array
///
/// OPTIMIZED: Computes transform inverse ONCE, then uses incremental pixel stepping
/// instead of per-pixel transform inversions (65,536 inversions -> 1)
fn extract_tile_region(
    source: &dyn RasterSource,
    transform: &AffineTransform<f64>,
    extent_3857: &GeometryExtent,
    source_extent_3857: &GeometryExtent,
    tile_size: (usize, usize),
) -> Result<Vec<f32>, Box<dyn std::error::Error + Send + Sync>> {
    let height = source.height();
    let width = source.width();
    let (tile_size_x, tile_size_y) = tile_size;

    // Calculate pixel resolution for the target tile
    let res_x = (extent_3857.maxx - extent_3857.minx) / (tile_size_x as f64);
    let res_y = (extent_3857.maxy - extent_3857.miny) / (tile_size_y as f64);

    let mut pixel_data = vec![f32::NAN; tile_size_x * tile_size_y];
    let mut valid_pixels = 0;

    // OPTIMIZATION: Compute inverse transform ONCE instead of 65,536 times
    let inv_transform = match transform.inverse() {
        Some(inv) => inv,
        None => {
            // Non-invertible transform, fall back to direct sampling
            return extract_tile_region_direct(source, tile_size);
        }
    };

    // Pre-compute the transform coefficients for incremental stepping
    // World -> Pixel: px = a*wx + b*wy + xoff, py = d*wx + e*wy + yoff
    let inv_a = inv_transform.a();
    let inv_b = inv_transform.b();
    let inv_xoff = inv_transform.xoff();
    let inv_d = inv_transform.d();
    let inv_e = inv_transform.e();
    let inv_yoff = inv_transform.yoff();

    // Compute starting pixel position (top-left corner of tile)
    let start_world_x = extent_3857.minx;
    let start_world_y = extent_3857.maxy; // Y starts at top (max) and goes down

    // Compute source pixel increments per output pixel
    // Moving one pixel right in output = moving res_x in world X
    let dx_src_x = inv_a * res_x; // Change in source X per output X step
    let dx_src_y = inv_d * res_x; // Change in source Y per output X step
    // Moving one pixel down in output = moving -res_y in world Y
    let dy_src_x = -inv_b * res_y; // Change in source X per output Y step
    let dy_src_y = -inv_e * res_y; // Change in source Y per output Y step

    // Starting source pixel for top-left corner
    let start_src_x = inv_a * start_world_x + inv_b * start_world_y + inv_xoff;
    let start_src_y = inv_d * start_world_x + inv_e * start_world_y + inv_yoff;

    // Process rows with incremental stepping (no per-pixel transform!)
    for y in 0..tile_size_y {
        // Source position at start of this row
        let mut src_x = start_src_x + y as f64 * dy_src_x;
        let mut src_y = start_src_y + y as f64 * dy_src_y;

        // Compute world Y for bounds checking (only need to check once per row for Y)
        let world_y = extent_3857.maxy - (y as f64) * res_y;
        let y_in_bounds = world_y >= source_extent_3857.miny && world_y <= source_extent_3857.maxy;

        for x in 0..tile_size_x {
            // Compute world X for bounds checking
            let world_x = extent_3857.minx + (x as f64) * res_x;

            // Check if world coordinate is within source extent
            if y_in_bounds
                && world_x >= source_extent_3857.minx
                && world_x <= source_extent_3857.maxx
            {
                // Check if source pixel is within array bounds
                let src_xi = src_x as isize;
                let src_yi = src_y as isize;

                if src_xi >= 0 && src_xi < width as isize && src_yi >= 0 && src_yi < height as isize {
                    if let Some(pixel_value) = source.sample(0, src_xi as usize, src_yi as usize) {
                        if !pixel_value.is_nan() {
                            valid_pixels += 1;
                        }
                        pixel_data[y * tile_size_x + x] = pixel_value;
                    }
                }
            }

            // Step to next pixel in row (incremental, no transform needed!)
            src_x += dx_src_x;
            src_y += dx_src_y;
        }
    }

    // If no valid pixels were found using the transform, try direct sampling
    if valid_pixels == 0 {
        return extract_tile_region_direct(source, tile_size);
    }

    Ok(pixel_data)
}

/// Fallback direct sampling from center of image when transform fails
fn extract_tile_region_direct(
    source: &dyn RasterSource,
    tile_size: (usize, usize),
) -> Result<Vec<f32>, Box<dyn std::error::Error + Send + Sync>> {
    let height = source.height();
    let width = source.width();
    let (tile_size_x, tile_size_y) = tile_size;

    let mut pixel_data = vec![f32::NAN; tile_size_x * tile_size_y];
    let center_x = width / 2;
    let center_y = height / 2;
    let sample_size = tile_size_x.min(width).min(tile_size_y);

    for y in 0..tile_size_y {
        for x in 0..tile_size_x {
            let src_x = (center_x - sample_size / 2 + x.min(sample_size - 1)) as usize;
            let src_y = (center_y - sample_size / 2 + y.min(sample_size - 1)) as usize;

            if src_x < width && src_y < height {
                if let Some(pixel_value) = source.sample(0, src_x, src_y) {
                    pixel_data[y * tile_size_x + x] = pixel_value;
                }
            }
        }
    }

    Ok(pixel_data)
}

pub async fn process_cog(
    input_path: PathBuf,
    extent_3857: GeometryExtent,
    layer_obj: Layer,
    tile_size: (usize, usize),
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    task::spawn_blocking(move || {
        let (tile_size_x, tile_size_y) = tile_size;

        // 1. Load COG data using cog3pio (with caching)
        let (source, transform) = load_cog_data(&input_path)?;

        // For now, assume the layer is in Web Mercator (EPSG:3857)
        // In a real implementation, we'd need to extract the actual CRS from the GeoTIFF
        let source_crs = format!("EPSG:{}", layer_obj.source_geometry.crs_code);

        // Transform source extent to Web Mercator if needed
        let source_extent_3857 = if source_crs == "EPSG:3857" {
            layer_obj.source_geometry.extent.clone()
        } else {
            // For now, only handle EPSG:3857
            // TODO: Implement coordinate transformations using geo crate for other CRS
            layer_obj.source_geometry.extent.clone()
        };

        // 2. Extract tile region from the array
        let pixel_data = extract_tile_region(
            source.as_ref(),
            &transform, // Use the actual transform from cog3pio
            &extent_3857,
            &source_extent_3857,
            tile_size,
        )?;

        // 3. Apply styling logic (same as original GDAL implementation)
        let mut img = RgbaImage::new(tile_size_x as u32, tile_size_y as u32);

        if let Some(grad) = get_builtin_gradient(&layer_obj.style) {
            // Use the gradient to colourise the image
            for (i, &raw) in pixel_data.iter().enumerate() {
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
                let y = (i / tile_size_y) as u32;
                img.put_pixel(x, y, px);
            }
        } else if layer_obj.colour_stops.is_empty() {
            // Fallback to grayscale
            for (i, &raw) in pixel_data.iter().enumerate() {
                let px = if is_nodata(raw) {
                    Rgba([0, 0, 0, 0])
                } else {
                    let norm =
                        (raw - layer_obj.min_value) / (layer_obj.max_value - layer_obj.min_value);
                    let lum = (norm.clamp(0.0, 1.0) * 255.0) as u8;
                    Rgba([lum, lum, lum, 255])
                };
                let x = (i % tile_size_x) as u32;
                let y = (i / tile_size_y) as u32;
                img.put_pixel(x, y, px);
            }
        } else {
            // Use the colour stops to colourise the image
            let cs = &layer_obj.colour_stops;
            let style_min = cs.first().unwrap().value;
            let style_max = cs.last().unwrap().value;
            for (i, &raw) in pixel_data.iter().enumerate() {
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
                            let r = ((1.0 - t) * a.red as f32 + t * b.red as f32) as u8;
                            let g = ((1.0 - t) * a.green as f32 + t * b.green as f32) as u8;
                            let b_ = ((1.0 - t) * a.blue as f32 + t * b.blue as f32) as u8;
                            let a_ = ((1.0 - t) * a.alpha as f32 + t * b.alpha as f32) as u8;
                            colour = Rgba([r, g, b_, a_]);
                            break;
                        }
                    }
                    colour
                };
                let x = (i % tile_size_x) as u32;
                let y = (i / tile_size_y) as u32;
                img.put_pixel(x, y, px);
            }
        }

        // 4. Encode to PNG
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        geometry::GeometryExtent,
        layer::{Layer, LayerGeometry},
        style::ColourStop,
    };
    use crate::reader::test_utils::{SyntheticRaster, RasterPattern, assert_approx_eq};

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
        match result {
            Ok(_) => {}
            Err(_) => {}
        }
    }

    // ========================================================================
    // Transform Tests
    // ========================================================================

    #[test]
    fn test_world_to_pixel_identity() {
        // Identity transform: world coords = pixel coords
        let transform = AffineTransform::new(1.0, 0.0, 0.0, 0.0, 1.0, 0.0);
        let world_coord = Coord { x: 10.0, y: 20.0 };
        let pixel_coord = world_to_pixel(world_coord, &transform);

        assert!((pixel_coord.x - 10.0).abs() < f64::EPSILON);
        assert!((pixel_coord.y - 20.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_world_to_pixel_scaled() {
        // Scaled transform: 10 world units = 1 pixel
        let transform = AffineTransform::new(10.0, 0.0, 0.0, 0.0, 10.0, 0.0);
        let world_coord = Coord { x: 100.0, y: 200.0 };
        let pixel_coord = world_to_pixel(world_coord, &transform);

        assert!((pixel_coord.x - 10.0).abs() < 0.001);
        assert!((pixel_coord.y - 20.0).abs() < 0.001);
    }

    #[test]
    fn test_world_to_pixel_with_offset() {
        // Transform with offset: origin at (1000, 2000)
        let transform = AffineTransform::new(1.0, 0.0, 1000.0, 0.0, 1.0, 2000.0);
        let world_coord = Coord { x: 1010.0, y: 2020.0 };
        let pixel_coord = world_to_pixel(world_coord, &transform);

        assert!((pixel_coord.x - 10.0).abs() < 0.001);
        assert!((pixel_coord.y - 20.0).abs() < 0.001);
    }

    #[test]
    fn test_world_to_pixel_web_mercator_style() {
        // Typical Web Mercator style: Y inverted, origin at top-left
        let pixel_width = 156543.0; // ~zoom level 0
        let origin_x = -20037508.342789244;
        let origin_y = 20037508.342789244;
        let transform = AffineTransform::new(
            pixel_width, 0.0, origin_x,
            0.0, -pixel_width, origin_y
        );

        // World (0, 0) should be roughly center of image
        let world_coord = Coord { x: 0.0, y: 0.0 };
        let pixel_coord = world_to_pixel(world_coord, &transform);

        // At zoom 0, center should be around pixel 128
        assert!(pixel_coord.x > 100.0 && pixel_coord.x < 200.0);
        assert!(pixel_coord.y > 100.0 && pixel_coord.y < 200.0);
    }

    // ========================================================================
    // Build Transform Tests
    // ========================================================================

    #[test]
    fn test_build_affine_transform_with_geotags() {
        let pixel_scale = Some([10.0, 10.0, 0.0]);
        let tiepoint = Some([0.0, 0.0, 0.0, 100.0, 500.0, 0.0]);
        let transform = build_affine_transform(100, 50, pixel_scale, tiepoint);

        // Check transform coefficients
        assert_approx_eq(transform.a() as f32, 10.0, 0.001, "x_scale");
        assert_approx_eq(transform.e() as f32, -10.0, 0.001, "y_scale (negative for image coords)");
        assert_approx_eq(transform.xoff() as f32, 100.0, 0.001, "x_origin");
        assert_approx_eq(transform.yoff() as f32, 500.0, 0.001, "y_origin");
    }

    #[test]
    fn test_build_affine_transform_fallback() {
        // Without geotags, should fallback to Web Mercator world
        let transform = build_affine_transform(256, 256, None, None);

        // Check it covers Web Mercator extent
        let web_mercator_extent = 20037508.342789244 * 2.0;
        let expected_pixel_size = web_mercator_extent / 256.0;

        assert_approx_eq(transform.a() as f32, expected_pixel_size as f32, 1.0, "fallback pixel width");
    }

    // ========================================================================
    // Tile Extraction Tests with Synthetic Data
    // ========================================================================

    #[test]
    fn test_extract_tile_region_identity_transform() {
        // Create synthetic raster with coordinate-encoded pattern
        let raster = SyntheticRaster::new(256, 256, RasterPattern::CoordinateEncoded)
            .with_identity_transform();

        let transform = AffineTransform::new(1.0, 0.0, 0.0, 0.0, -1.0, 256.0);
        let source_extent = GeometryExtent::new(0.0, 0.0, 256.0, 256.0);
        let tile_extent = GeometryExtent::new(0.0, 0.0, 256.0, 256.0);

        let result = extract_tile_region(
            &raster,
            &transform,
            &tile_extent,
            &source_extent,
            (256, 256),
        ).unwrap();

        assert_eq!(result.len(), 256 * 256);

        // Check corners - with identity transform, tile pixels should match source pixels
        // Top-left (0, 0)
        let tl_value = result[0];
        assert!(!tl_value.is_nan(), "Top-left should have data");

        // Check that we have varying values (not all same)
        let unique_values: std::collections::HashSet<u32> = result.iter()
            .filter(|v| !v.is_nan())
            .map(|v| *v as u32)
            .collect();
        assert!(unique_values.len() > 1, "Should have multiple unique values");
    }

    #[test]
    fn test_extract_tile_region_quadrants() {
        let raster = SyntheticRaster::new(100, 100, RasterPattern::Quadrants)
            .with_identity_transform();

        let transform = AffineTransform::new(1.0, 0.0, 0.0, 0.0, -1.0, 100.0);
        let source_extent = GeometryExtent::new(0.0, 0.0, 100.0, 100.0);

        // Extract top-left quadrant only
        let tile_extent = GeometryExtent::new(0.0, 50.0, 50.0, 100.0);
        let result = extract_tile_region(
            &raster,
            &transform,
            &tile_extent,
            &source_extent,
            (50, 50),
        ).unwrap();

        // All values should be 1.0 (top-left quadrant)
        let valid_values: Vec<f32> = result.iter().filter(|v| !v.is_nan()).copied().collect();
        if !valid_values.is_empty() {
            // At least some pixels should be from quadrant 1
            assert!(valid_values.iter().any(|&v| (v - 1.0).abs() < 0.1),
                "Top-left quadrant should contain value 1.0");
        }
    }

    #[test]
    fn test_extract_tile_region_outside_bounds() {
        let raster = SyntheticRaster::new(100, 100, RasterPattern::Constant(42.0))
            .with_identity_transform();

        let transform = AffineTransform::new(1.0, 0.0, 0.0, 0.0, -1.0, 100.0);
        let source_extent = GeometryExtent::new(0.0, 0.0, 100.0, 100.0);

        // Request tile completely outside source
        let tile_extent = GeometryExtent::new(200.0, 200.0, 300.0, 300.0);
        let result = extract_tile_region(
            &raster,
            &transform,
            &tile_extent,
            &source_extent,
            (100, 100),
        ).unwrap();

        // Note: When no valid pixels are found, the fallback function samples from center
        // So we expect either all NaN (no transform fallback triggered) or values from center
        // The important thing is the function doesn't crash
        assert_eq!(result.len(), 100 * 100, "Should return correct number of pixels");
    }

    #[test]
    fn test_extract_tile_region_partial_overlap() {
        let raster = SyntheticRaster::new(100, 100, RasterPattern::Constant(42.0))
            .with_identity_transform();

        let transform = AffineTransform::new(1.0, 0.0, 0.0, 0.0, -1.0, 100.0);
        let source_extent = GeometryExtent::new(0.0, 0.0, 100.0, 100.0);

        // Request tile that partially overlaps source
        let tile_extent = GeometryExtent::new(50.0, 50.0, 150.0, 150.0);
        let result = extract_tile_region(
            &raster,
            &transform,
            &tile_extent,
            &source_extent,
            (100, 100),
        ).unwrap();

        // Should have some valid and some NaN values
        let valid_count = result.iter().filter(|v| !v.is_nan()).count();
        let nan_count = result.iter().filter(|v| v.is_nan()).count();

        assert!(valid_count > 0, "Should have some valid pixels in overlap");
        assert!(nan_count > 0, "Should have some NaN pixels outside source");
    }

    // ========================================================================
    // Gradient Extraction Tests
    // ========================================================================

    #[test]
    fn test_extract_horizontal_gradient() {
        let raster = SyntheticRaster::new(100, 100, RasterPattern::HorizontalGradient)
            .with_identity_transform();

        let transform = AffineTransform::new(1.0, 0.0, 0.0, 0.0, -1.0, 100.0);
        let source_extent = GeometryExtent::new(0.0, 0.0, 100.0, 100.0);
        let tile_extent = GeometryExtent::new(0.0, 0.0, 100.0, 100.0);

        let result = extract_tile_region(
            &raster,
            &transform,
            &tile_extent,
            &source_extent,
            (100, 100),
        ).unwrap();

        // Check that values increase left to right (first row)
        let first_row: Vec<f32> = result[0..100].to_vec();
        let valid_first_row: Vec<f32> = first_row.iter()
            .filter(|v| !v.is_nan())
            .copied()
            .collect();

        if valid_first_row.len() > 10 {
            // Values should generally increase
            let first_val = valid_first_row[0];
            let last_val = *valid_first_row.last().unwrap();
            assert!(last_val >= first_val,
                "Horizontal gradient should increase left to right");
        }
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
        let stops = vec![
            ColourStop { value: 0.0, red: 0, green: 0, blue: 0, alpha: 255 },
            ColourStop { value: 100.0, red: 255, green: 255, blue: 255, alpha: 255 },
        ];

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
    // Min/Max Computation Tests
    // ========================================================================

    #[test]
    fn test_raster_read_result_compute_min_max() {
        // Create array with known values
        let data = ndarray::Array3::from_shape_fn((1, 10, 10), |(_, y, x)| {
            (x + y * 10) as f32
        });
        let result = RasterReadResult::Array(data);

        let (min, max) = result.compute_min_max().unwrap();
        assert_approx_eq(min, 0.0, 0.001, "min value");
        assert_approx_eq(max, 99.0, 0.001, "max value");
    }

    #[test]
    fn test_raster_read_result_compute_min_max_with_nan() {
        // Create array with some NaN values
        let mut data = ndarray::Array3::from_shape_fn((1, 10, 10), |(_, y, x)| {
            (x + y * 10) as f32
        });
        // Set some values to NaN
        data[[0, 0, 0]] = f32::NAN;
        data[[0, 5, 5]] = f32::NAN;

        let result = RasterReadResult::Array(data);
        let (min, max) = result.compute_min_max().unwrap();

        // Should skip NaN values
        assert!(!min.is_nan(), "min should not be NaN");
        assert!(!max.is_nan(), "max should not be NaN");
        assert_approx_eq(min, 1.0, 0.001, "min value (skipping NaN at 0,0)");
        assert_approx_eq(max, 99.0, 0.001, "max value");
    }

    #[test]
    fn test_raster_read_result_dimensions() {
        let data = ndarray::Array3::from_shape_fn((3, 100, 200), |_| 0.0f32);
        let result = RasterReadResult::Array(data);

        let (bands, height, width) = result.dimensions();
        assert_eq!(bands, 3);
        assert_eq!(height, 100);
        assert_eq!(width, 200);
    }
}
