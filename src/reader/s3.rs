//! S3 Range Reader implementation using `object_store`
//!
//! This module provides S3-compatible storage access for reading COG files.
//! It supports:
//! - AWS S3
//! - MinIO
//! - Any S3-compatible storage (DigitalOcean Spaces, Backblaze B2, etc.)
//!
//! # Configuration
//!
//! The reader can be configured via environment variables:
//! - `AWS_ACCESS_KEY_ID` - AWS access key
//! - `AWS_SECRET_ACCESS_KEY` - AWS secret key
//! - `AWS_REGION` - AWS region (default: us-east-1)
//! - `AWS_ENDPOINT_URL` - Custom endpoint for MinIO/S3-compatible services
//! - `AWS_ALLOW_HTTP` - Set to "true" to allow HTTP endpoints (for local MinIO)
//!
//! # Example
//!
//! ```rust,no_run
//! use tileyolo::reader::s3::S3RangeReaderAsync;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//!     // For AWS S3
//!     let reader = S3RangeReaderAsync::new("s3://my-bucket/path/to/file.tif").await?;
//!
//!     // For MinIO (set AWS_ENDPOINT_URL=http://localhost:9000)
//!     // std::env::set_var("AWS_ENDPOINT_URL", "http://localhost:9000");
//!     // std::env::set_var("AWS_ALLOW_HTTP", "true");
//!     // let reader = S3RangeReaderAsync::new("s3://my-bucket/path/to/file.tif").await?;
//!
//!     Ok(())
//! }
//! ```

use crate::reader::range_reader::RangeReader;
use crate::reader::tiff_utils::AnyResult;
use object_store::aws::AmazonS3Builder;
use object_store::path::Path as ObjectPath;
use object_store::{GetOptions, GetRange, ObjectStore};
use std::ops::Range;
use std::sync::Arc;
use tokio::runtime::Handle;

/// S3 configuration for connecting to S3-compatible storage
#[derive(Debug, Clone)]
pub struct S3Config {
    /// S3 bucket name
    pub bucket: String,
    /// Object key (path within the bucket)
    pub key: String,
    /// AWS region (default: us-east-1)
    pub region: Option<String>,
    /// Custom endpoint URL (for MinIO, LocalStack, etc.)
    pub endpoint_url: Option<String>,
    /// AWS access key ID
    pub access_key_id: Option<String>,
    /// AWS secret access key
    pub secret_access_key: Option<String>,
    /// Allow HTTP connections (required for local MinIO without TLS)
    pub allow_http: bool,
    /// Skip signature verification (for anonymous access to public buckets)
    pub skip_signature: bool,
}

impl S3Config {
    /// Create a new S3 config from an S3 URL
    ///
    /// Parses URLs like `s3://bucket/key/path`
    pub fn from_url(url: &str) -> AnyResult<Self> {
        let parsed = url::Url::parse(url)?;

        if parsed.scheme() != "s3" {
            return Err(format!("Expected s3:// URL, got: {}", parsed.scheme()).into());
        }

        let bucket = parsed
            .host_str()
            .ok_or("Missing bucket in S3 URL")?
            .to_string();

        let key = parsed.path().trim_start_matches('/').to_string();

        if key.is_empty() {
            return Err("Missing key in S3 URL".into());
        }

        Ok(Self {
            bucket,
            key,
            region: std::env::var("AWS_REGION").ok().or(Some("us-east-1".to_string())),
            endpoint_url: std::env::var("AWS_ENDPOINT_URL").ok(),
            access_key_id: std::env::var("AWS_ACCESS_KEY_ID").ok(),
            secret_access_key: std::env::var("AWS_SECRET_ACCESS_KEY").ok(),
            allow_http: std::env::var("AWS_ALLOW_HTTP")
                .map(|v| v.to_lowercase() == "true")
                .unwrap_or(false),
            skip_signature: std::env::var("AWS_SKIP_SIGNATURE")
                .map(|v| v.to_lowercase() == "true")
                .unwrap_or(false),
        })
    }

    /// Create a config for MinIO with default local settings
    pub fn for_minio(bucket: &str, key: &str, endpoint: &str) -> Self {
        Self {
            bucket: bucket.to_string(),
            key: key.to_string(),
            region: Some("us-east-1".to_string()),
            endpoint_url: Some(endpoint.to_string()),
            access_key_id: std::env::var("AWS_ACCESS_KEY_ID").ok(),
            secret_access_key: std::env::var("AWS_SECRET_ACCESS_KEY").ok(),
            allow_http: true,
            skip_signature: false,
        }
    }
}

/// Async S3 range reader using `object_store`
pub struct S3RangeReaderAsync {
    store: Arc<dyn ObjectStore>,
    path: ObjectPath,
    size: u64,
    url: String,
}

impl S3RangeReaderAsync {
    /// Create a new S3 range reader from an S3 URL
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use tileyolo::reader::s3::S3RangeReaderAsync;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    /// let reader = S3RangeReaderAsync::new("s3://my-bucket/data/file.tif").await?;
    /// println!("File size: {} bytes", reader.size());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn new(url: &str) -> AnyResult<Self> {
        let config = S3Config::from_url(url)?;
        Self::from_config(config).await
    }

    /// Create a new S3 range reader from a config
    pub async fn from_config(config: S3Config) -> AnyResult<Self> {
        let mut builder = AmazonS3Builder::new()
            .with_bucket_name(&config.bucket);

        if let Some(region) = &config.region {
            builder = builder.with_region(region);
        }

        if let Some(endpoint) = &config.endpoint_url {
            builder = builder.with_endpoint(endpoint);
        }

        if let Some(access_key) = &config.access_key_id {
            builder = builder.with_access_key_id(access_key);
        }

        if let Some(secret_key) = &config.secret_access_key {
            builder = builder.with_secret_access_key(secret_key);
        }

        if config.allow_http {
            builder = builder.with_allow_http(true);
        }

        if config.skip_signature {
            builder = builder.with_skip_signature(true);
        }

        let store = builder.build()?;
        let path = ObjectPath::from(config.key.as_str());

        // Get file size via HEAD request
        let meta = store.head(&path).await?;
        let size = meta.size as u64;

        let url = format!("s3://{}/{}", config.bucket, config.key);

        Ok(Self {
            store: Arc::new(store),
            path,
            size,
            url,
        })
    }

    /// Read a range of bytes asynchronously
    pub async fn read_range_async(&self, offset: u64, length: usize) -> AnyResult<Vec<u8>> {
        let range = Range {
            start: offset,
            end: offset + length as u64,
        };

        let options = GetOptions {
            range: Some(GetRange::Bounded(range)),
            ..Default::default()
        };

        let result = self.store.get_opts(&self.path, options).await?;
        let bytes = result.bytes().await?;

        Ok(bytes.to_vec())
    }

    /// Get the file size
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Get the S3 URL
    pub fn url(&self) -> &str {
        &self.url
    }
}

/// Synchronous wrapper for S3RangeReaderAsync that implements RangeReader trait
pub struct S3RangeReaderSync {
    inner: S3RangeReaderAsync,
    runtime: Handle,
}

impl S3RangeReaderSync {
    /// Create a new sync S3 range reader
    ///
    /// Must be called from within a tokio runtime context
    pub fn new(url: &str) -> AnyResult<Self> {
        let runtime = Handle::try_current()
            .map_err(|_| "S3RangeReaderSync must be created within a tokio runtime")?;

        let inner = runtime.block_on(S3RangeReaderAsync::new(url))?;

        Ok(Self { inner, runtime })
    }

    /// Create from an existing async reader
    pub fn from_async(inner: S3RangeReaderAsync) -> AnyResult<Self> {
        let runtime = Handle::try_current()
            .map_err(|_| "S3RangeReaderSync must be created within a tokio runtime")?;

        Ok(Self { inner, runtime })
    }
}

impl RangeReader for S3RangeReaderSync {
    fn read_range(&self, offset: u64, length: usize) -> AnyResult<Vec<u8>> {
        self.runtime.block_on(self.inner.read_range_async(offset, length))
    }

    fn size(&self) -> u64 {
        self.inner.size
    }

    fn identifier(&self) -> &str {
        &self.inner.url
    }

    fn is_local(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_s3_config_from_url() {
        let config = S3Config::from_url("s3://my-bucket/path/to/file.tif").unwrap();
        assert_eq!(config.bucket, "my-bucket");
        assert_eq!(config.key, "path/to/file.tif");
    }

    #[test]
    fn test_s3_config_from_url_simple() {
        let config = S3Config::from_url("s3://bucket/file.tif").unwrap();
        assert_eq!(config.bucket, "bucket");
        assert_eq!(config.key, "file.tif");
    }

    #[test]
    fn test_s3_config_invalid_scheme() {
        let result = S3Config::from_url("http://bucket/file.tif");
        assert!(result.is_err());
    }

    #[test]
    fn test_s3_config_missing_key() {
        let result = S3Config::from_url("s3://bucket/");
        assert!(result.is_err());
    }

    #[test]
    fn test_minio_config() {
        let config = S3Config::for_minio("test-bucket", "data/test.tif", "http://localhost:9000");
        assert_eq!(config.bucket, "test-bucket");
        assert_eq!(config.key, "data/test.tif");
        assert_eq!(config.endpoint_url, Some("http://localhost:9000".to_string()));
        assert!(config.allow_http);
    }
}
