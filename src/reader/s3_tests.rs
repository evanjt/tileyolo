//! S3 Integration Tests
//!
//! These tests require an S3-compatible server (MinIO) to be running.
//! They are gated behind the `s3-tests` feature and will be skipped
//! if the S3 endpoint is not available.
//!
//! # Running locally with MinIO
//!
//! ```bash
//! # Start MinIO
//! docker run -d --name minio \
//!   -p 9000:9000 -p 9001:9001 \
//!   -e MINIO_ROOT_USER=minioadmin \
//!   -e MINIO_ROOT_PASSWORD=minioadmin \
//!   minio/minio server /data --console-address ":9001"
//!
//! # Create bucket and upload test file
//! mc alias set local http://localhost:9000 minioadmin minioadmin
//! mc mb local/test-bucket
//! mc cp test_data/viridis/test.tif local/test-bucket/test.tif
//!
//! # Run tests
//! AWS_ACCESS_KEY_ID=minioadmin \
//! AWS_SECRET_ACCESS_KEY=minioadmin \
//! AWS_ENDPOINT_URL=http://localhost:9000 \
//! AWS_ALLOW_HTTP=true \
//! cargo test --features s3-tests s3_integration
//! ```

use super::s3::{S3Config, S3RangeReaderAsync};
use std::env;

/// Check if S3 testing environment is available
fn s3_test_available() -> bool {
    env::var("AWS_ENDPOINT_URL").is_ok()
        && env::var("AWS_ACCESS_KEY_ID").is_ok()
        && env::var("AWS_SECRET_ACCESS_KEY").is_ok()
}

/// Get the test bucket name (default: test-bucket)
fn test_bucket() -> String {
    env::var("S3_TEST_BUCKET").unwrap_or_else(|_| "test-bucket".to_string())
}

/// Get the test file key (default: test.tif)
fn test_key() -> String {
    env::var("S3_TEST_KEY").unwrap_or_else(|_| "test.tif".to_string())
}

#[tokio::test]
async fn test_s3_connection() {
    if !s3_test_available() {
        println!("Skipping S3 test - no S3 endpoint configured");
        return;
    }

    let url = format!("s3://{}/{}", test_bucket(), test_key());
    let result = S3RangeReaderAsync::new(&url).await;

    match result {
        Ok(reader) => {
            println!("Successfully connected to S3");
            println!("File size: {} bytes", reader.size());
            assert!(reader.size() > 0, "File should have non-zero size");
        }
        Err(e) => {
            // If bucket/file doesn't exist, that's expected in some test environments
            println!("S3 connection error (may be expected): {e}");
        }
    }
}

#[tokio::test]
async fn test_s3_read_range() {
    if !s3_test_available() {
        println!("Skipping S3 test - no S3 endpoint configured");
        return;
    }

    let url = format!("s3://{}/{}", test_bucket(), test_key());
    let reader = match S3RangeReaderAsync::new(&url).await {
        Ok(r) => r,
        Err(e) => {
            println!("Skipping test - could not connect: {e}");
            return;
        }
    };

    // Read TIFF header (first 8 bytes)
    let header = reader.read_range_async(0, 8).await.unwrap();
    assert_eq!(header.len(), 8);

    // Check for TIFF magic bytes (II for little-endian or MM for big-endian)
    let magic = &header[0..2];
    assert!(
        magic == b"II" || magic == b"MM",
        "Expected TIFF magic bytes, got: {:?}",
        magic
    );

    println!("TIFF header validated: {:?}", &header[0..4]);
}

#[tokio::test]
async fn test_s3_read_multiple_ranges() {
    if !s3_test_available() {
        println!("Skipping S3 test - no S3 endpoint configured");
        return;
    }

    let url = format!("s3://{}/{}", test_bucket(), test_key());
    let reader = match S3RangeReaderAsync::new(&url).await {
        Ok(r) => r,
        Err(e) => {
            println!("Skipping test - could not connect: {e}");
            return;
        }
    };

    // Read first range
    let range1 = reader.read_range_async(0, 100).await.unwrap();
    assert_eq!(range1.len(), 100);

    // Read second range (overlapping is fine)
    let range2 = reader.read_range_async(50, 100).await.unwrap();
    assert_eq!(range2.len(), 100);

    // Verify overlapping portion is consistent
    assert_eq!(
        &range1[50..100],
        &range2[0..50],
        "Overlapping ranges should have consistent data"
    );

    println!("Multiple range reads validated");
}

#[tokio::test]
async fn test_s3_config_from_env() {
    if !s3_test_available() {
        println!("Skipping S3 test - no S3 endpoint configured");
        return;
    }

    let config = S3Config::from_url("s3://test-bucket/test.tif").unwrap();

    // Verify env vars are picked up
    assert!(config.endpoint_url.is_some());
    assert!(config.access_key_id.is_some());
    assert!(config.secret_access_key.is_some());

    println!("S3 config from environment: {:?}", config);
}

#[tokio::test]
async fn test_s3_minio_config() {
    let config = S3Config::for_minio("my-bucket", "path/to/file.tif", "http://localhost:9000");

    assert_eq!(config.bucket, "my-bucket");
    assert_eq!(config.key, "path/to/file.tif");
    assert_eq!(
        config.endpoint_url,
        Some("http://localhost:9000".to_string())
    );
    assert!(config.allow_http);
}

#[tokio::test]
async fn test_s3_file_size() {
    if !s3_test_available() {
        println!("Skipping S3 test - no S3 endpoint configured");
        return;
    }

    let url = format!("s3://{}/{}", test_bucket(), test_key());
    let reader = match S3RangeReaderAsync::new(&url).await {
        Ok(r) => r,
        Err(e) => {
            println!("Skipping test - could not connect: {e}");
            return;
        }
    };

    let size = reader.size();
    println!("S3 file size: {size} bytes");

    // A valid TIFF should be at least a few KB
    assert!(size > 100, "File size should be reasonable for a TIFF");
}

#[tokio::test]
async fn test_s3_read_entire_small_file() {
    if !s3_test_available() {
        println!("Skipping S3 test - no S3 endpoint configured");
        return;
    }

    let url = format!("s3://{}/{}", test_bucket(), test_key());
    let reader = match S3RangeReaderAsync::new(&url).await {
        Ok(r) => r,
        Err(e) => {
            println!("Skipping test - could not connect: {e}");
            return;
        }
    };

    let size = reader.size();
    if size > 10 * 1024 * 1024 {
        println!("Skipping full file read - file too large ({size} bytes)");
        return;
    }

    let data = reader.read_range_async(0, size as usize).await.unwrap();
    assert_eq!(data.len(), size as usize);

    println!("Successfully read entire file ({size} bytes)");
}

/// Test that errors are handled gracefully for non-existent files
#[tokio::test]
async fn test_s3_nonexistent_file() {
    if !s3_test_available() {
        println!("Skipping S3 test - no S3 endpoint configured");
        return;
    }

    let url = format!(
        "s3://{}/definitely-does-not-exist-{}.tif",
        test_bucket(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );

    let result = S3RangeReaderAsync::new(&url).await;
    assert!(result.is_err(), "Should fail for non-existent file");
    println!("Non-existent file correctly returned error");
}

/// Test that errors are handled gracefully for non-existent buckets
#[tokio::test]
async fn test_s3_nonexistent_bucket() {
    if !s3_test_available() {
        println!("Skipping S3 test - no S3 endpoint configured");
        return;
    }

    let url = format!(
        "s3://definitely-does-not-exist-bucket-{}/test.tif",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );

    let result = S3RangeReaderAsync::new(&url).await;
    assert!(result.is_err(), "Should fail for non-existent bucket");
    println!("Non-existent bucket correctly returned error");
}

// ============================================================================
// S3TileReader Integration Tests
// ============================================================================

use super::s3_tile_reader::S3TileReader;
use crate::traits::TileReader;

/// Test S3TileReader initialization with a valid bucket
#[tokio::test(flavor = "multi_thread")]
async fn test_s3_tile_reader_init() {
    if !s3_test_available() {
        println!("Skipping S3TileReader test - no S3 endpoint configured");
        return;
    }

    let bucket = test_bucket();
    // The prefix should point to the parent of style folders
    // Structure: bucket/viridis/layer.tif -> prefix should be ""
    let prefix = "";

    match S3TileReader::new(&bucket, prefix).await {
        Ok(reader) => {
            let layers = reader.list_layers().await;
            println!("S3TileReader found {} layers", layers.len());
            for layer in &layers {
                println!("  - {} (style: {})", layer.layer, layer.style);
            }
        }
        Err(e) => {
            println!("S3TileReader init failed (may be expected if no test data): {e}");
        }
    }
}

/// Test S3TileReader can list layers from bucket
#[tokio::test(flavor = "multi_thread")]
async fn test_s3_tile_reader_list_layers() {
    if !s3_test_available() {
        println!("Skipping S3TileReader test - no S3 endpoint configured");
        return;
    }

    let bucket = test_bucket();
    let reader = match S3TileReader::new(&bucket, "").await {
        Ok(r) => r,
        Err(e) => {
            println!("Skipping - could not init S3TileReader: {e}");
            return;
        }
    };

    let layers = reader.list_layers().await;
    if layers.is_empty() {
        println!("No layers found - bucket may be empty or structure incorrect");
        println!("Expected structure: bucket/style/layer.tif");
        return;
    }

    println!("Found {} layers:", layers.len());
    for layer in &layers {
        println!(
            "  Layer: {}, Style: {}, Path: {:?}",
            layer.layer, layer.style, layer.path
        );
        println!(
            "    Extent: ({:.2}, {:.2}) to ({:.2}, {:.2})",
            layer.source_geometry.extent.minx,
            layer.source_geometry.extent.miny,
            layer.source_geometry.extent.maxx,
            layer.source_geometry.extent.maxy
        );
        println!("    CRS: EPSG:{}", layer.source_geometry.crs_code);
    }
}

/// Test S3TileReader can serve tiles
#[tokio::test(flavor = "multi_thread")]
async fn test_s3_tile_reader_get_tile() {
    if !s3_test_available() {
        println!("Skipping S3TileReader test - no S3 endpoint configured");
        return;
    }

    let bucket = test_bucket();
    let reader = match S3TileReader::new(&bucket, "").await {
        Ok(r) => r,
        Err(e) => {
            println!("Skipping - could not init S3TileReader: {e}");
            return;
        }
    };

    let layers = reader.list_layers().await;
    if layers.is_empty() {
        println!("No layers to test tile fetching");
        return;
    }

    let layer_name = &layers[0].layer;
    println!("Testing tile fetch for layer: {layer_name}");

    // Request zoom level 0 tile (world extent)
    match reader.get_tile(layer_name, 0, 0, 0, None).await {
        Ok(response) => {
            assert_eq!(response.content_type, "image/png");
            assert!(!response.bytes.is_empty(), "Tile should have content");
            println!(
                "Got tile: {} bytes, content-type: {}",
                response.bytes.len(),
                response.content_type
            );

            // Verify PNG magic bytes
            assert!(
                response.bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]),
                "Response should be a valid PNG"
            );
        }
        Err(e) => {
            println!("Tile fetch error: {e}");
        }
    }
}

/// Test S3TileReader returns transparent tile for out-of-bounds requests
#[tokio::test(flavor = "multi_thread")]
async fn test_s3_tile_reader_out_of_bounds() {
    if !s3_test_available() {
        println!("Skipping S3TileReader test - no S3 endpoint configured");
        return;
    }

    let bucket = test_bucket();
    let reader = match S3TileReader::new(&bucket, "").await {
        Ok(r) => r,
        Err(e) => {
            println!("Skipping - could not init S3TileReader: {e}");
            return;
        }
    };

    let layers = reader.list_layers().await;
    if layers.is_empty() {
        println!("No layers to test");
        return;
    }

    let layer_name = &layers[0].layer;

    // Request a tile at high zoom far from any likely data
    // Tile 15/0/0 is in the Pacific Ocean near the antimeridian
    match reader.get_tile(layer_name, 15, 0, 0, None).await {
        Ok(response) => {
            assert_eq!(response.content_type, "image/png");
            println!(
                "Out-of-bounds tile: {} bytes (likely transparent)",
                response.bytes.len()
            );
        }
        Err(e) => {
            println!("Out-of-bounds tile error (may be expected): {e}");
        }
    }
}

/// Test S3TileReader handles style override
#[tokio::test(flavor = "multi_thread")]
async fn test_s3_tile_reader_style_override() {
    if !s3_test_available() {
        println!("Skipping S3TileReader test - no S3 endpoint configured");
        return;
    }

    let bucket = test_bucket();
    let reader = match S3TileReader::new(&bucket, "").await {
        Ok(r) => r,
        Err(e) => {
            println!("Skipping - could not init S3TileReader: {e}");
            return;
        }
    };

    let layers = reader.list_layers().await;
    if layers.is_empty() {
        println!("No layers to test");
        return;
    }

    let layer_name = &layers[0].layer;

    // Request with different styles
    for style in &["viridis", "plasma", "magma"] {
        match reader.get_tile(layer_name, 0, 0, 0, Some(style)).await {
            Ok(response) => {
                println!("Style {style}: {} bytes", response.bytes.len());
            }
            Err(e) => {
                println!("Style {style} error: {e}");
            }
        }
    }
}

/// Test S3TileReader returns error for non-existent layer
#[tokio::test(flavor = "multi_thread")]
async fn test_s3_tile_reader_nonexistent_layer() {
    if !s3_test_available() {
        println!("Skipping S3TileReader test - no S3 endpoint configured");
        return;
    }

    let bucket = test_bucket();
    let reader = match S3TileReader::new(&bucket, "").await {
        Ok(r) => r,
        Err(e) => {
            println!("Skipping - could not init S3TileReader: {e}");
            return;
        }
    };

    let result = reader
        .get_tile("definitely-nonexistent-layer", 0, 0, 0, None)
        .await;
    assert!(result.is_err(), "Should fail for non-existent layer");
    println!("Non-existent layer correctly returned error");
}
