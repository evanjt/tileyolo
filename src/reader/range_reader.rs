//! Range-based reader interface for COG files
//!
//! This module provides a unified interface for reading byte ranges from various sources
//! (local files, S3, HTTP). This is essential for efficient COG reading since COGs are
//! designed to be read via HTTP Range requests.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::reader::tiff_utils::AnyResult;

/// Trait for reading byte ranges from any source
///
/// This abstraction allows the same COG reading code to work with:
/// - Local files (using seek + read)
/// - S3 objects (using `GetObject` with Range header)
/// - HTTP URLs (using Range header)
pub trait RangeReader: Send + Sync {
    /// Read a range of bytes from the source
    fn read_range(&self, offset: u64, length: usize) -> AnyResult<Vec<u8>>;

    /// Get the total size of the source in bytes
    fn size(&self) -> u64;

    /// Get a human-readable identifier for this source (for logging/errors)
    fn identifier(&self) -> &str;

    /// Check if this is a local file (fast random access) or remote (expensive reads)
    fn is_local(&self) -> bool {
        let id = self.identifier();
        !id.starts_with("http://") && !id.starts_with("https://") && !id.starts_with("s3://")
    }
}

/// Local file range reader
pub struct LocalRangeReader {
    path: PathBuf,
    size: u64,
}

impl LocalRangeReader {
    pub fn new(path: impl AsRef<Path>) -> AnyResult<Self> {
        let path = path.as_ref().to_path_buf();
        let metadata = std::fs::metadata(&path)?;
        Ok(Self {
            path,
            size: metadata.len(),
        })
    }
}

impl RangeReader for LocalRangeReader {
    fn read_range(&self, offset: u64, length: usize) -> AnyResult<Vec<u8>> {
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut buffer = vec![0u8; length];
        file.read_exact(&mut buffer)?;
        Ok(buffer)
    }

    fn size(&self) -> u64 {
        self.size
    }

    fn identifier(&self) -> &str {
        self.path.to_str().unwrap_or("<invalid path>")
    }
}

/// HTTP range reader for remote COG files
/// Uses reqwest with blocking client for simplicity in sync contexts
pub struct HttpRangeReader {
    url: String,
    size: u64,
    client: reqwest::blocking::Client,
}

impl HttpRangeReader {
    pub fn new(url: &str) -> AnyResult<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        // Get file size via HEAD request
        let response = client.head(url).send()?;
        let size = response
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        Ok(Self {
            url: url.to_string(),
            size,
            client,
        })
    }
}

impl RangeReader for HttpRangeReader {
    fn read_range(&self, offset: u64, length: usize) -> AnyResult<Vec<u8>> {
        let range = format!("bytes={}-{}", offset, offset + length as u64 - 1);
        let response = self.client
            .get(&self.url)
            .header("Range", range)
            .send()?;

        if !response.status().is_success() {
            return Err(format!("HTTP request failed: {}", response.status()).into());
        }

        Ok(response.bytes()?.to_vec())
    }

    fn size(&self) -> u64 {
        self.size
    }

    fn identifier(&self) -> &str {
        &self.url
    }
}

/// S3 range reader using `object_store`
pub struct S3RangeReader {
    #[allow(dead_code)]
    bucket: String,
    #[allow(dead_code)]
    key: String,
    size: u64,
    // We store the URL for identification
    url: String,
}

impl S3RangeReader {
    /// Create from an S3 URL like <s3://bucket/key>
    pub fn new(url: &str) -> AnyResult<Self> {
        // Parse s3://bucket/key format
        let url_parsed = url::Url::parse(url)?;

        if url_parsed.scheme() != "s3" {
            return Err("URL must use s3:// scheme".into());
        }

        let bucket = url_parsed.host_str()
            .ok_or("Missing bucket in S3 URL")?
            .to_string();

        let key = url_parsed.path().trim_start_matches('/').to_string();

        if key.is_empty() {
            return Err("Missing key in S3 URL".into());
        }

        // For now, return a placeholder - actual S3 implementation would use aws-sdk-s3
        // This is a simplified version that converts to HTTPS for public buckets
        Ok(Self {
            bucket,
            key,
            size: 0, // Would be fetched via HEAD
            url: url.to_string(),
        })
    }

    /// Create from an HTTPS URL pointing to S3-hosted content
    pub fn from_https(url: &str) -> AnyResult<Self> {
        // Use HTTP reader under the hood for public S3 buckets
        let http_reader = HttpRangeReader::new(url)?;

        Ok(Self {
            bucket: String::new(),
            key: String::new(),
            size: http_reader.size,
            url: url.to_string(),
        })
    }
}

impl RangeReader for S3RangeReader {
    fn read_range(&self, offset: u64, length: usize) -> AnyResult<Vec<u8>> {
        // For public S3 buckets, use HTTP range requests
        // For private buckets, this would use aws-sdk-s3 with credentials
        let client = reqwest::blocking::Client::new();
        let range = format!("bytes={}-{}", offset, offset + length as u64 - 1);

        let response = client
            .get(&self.url)
            .header("Range", range)
            .send()?;

        if !response.status().is_success() {
            return Err(format!("S3 request failed: {}", response.status()).into());
        }

        Ok(response.bytes()?.to_vec())
    }

    fn size(&self) -> u64 {
        self.size
    }

    fn identifier(&self) -> &str {
        &self.url
    }
}

/// Create a range reader from a path or URL
pub fn create_range_reader(source: &str) -> AnyResult<Arc<dyn RangeReader>> {
    if source.starts_with("s3://") {
        Ok(Arc::new(S3RangeReader::new(source)?))
    } else if source.starts_with("http://") || source.starts_with("https://") {
        Ok(Arc::new(HttpRangeReader::new(source)?))
    } else {
        Ok(Arc::new(LocalRangeReader::new(source)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_local_range_reader() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"Hello, World!").unwrap();

        let reader = LocalRangeReader::new(file.path()).unwrap();
        assert_eq!(reader.size(), 13);

        let data = reader.read_range(0, 5).unwrap();
        assert_eq!(&data, b"Hello");

        let data = reader.read_range(7, 5).unwrap();
        assert_eq!(&data, b"World");
    }
}
