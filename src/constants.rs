/// Web Mercator extent in meters (half the world).
/// This is the maximum extent of the EPSG:3857 projection.
pub const WEB_MERCATOR_EXTENT: f64 = 20_037_508.342_789_244;

/// Maximum number of cached COG file sources.
pub const MAX_CACHED_SOURCES: usize = 50;

/// Maximum number of cached COG readers.
pub const MAX_CACHED_COG_READERS: usize = 50;

/// Duration to cache failed file load attempts (in seconds).
pub const FAILURE_CACHE_DURATION_SECS: u64 = 300;
