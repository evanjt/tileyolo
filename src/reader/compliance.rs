//! COG Compliance Checker
//!
//! Validates `GeoTIFF` files against `TileYolo`'s requirements for optimal performance.
//! Reports issues and provides recommendations for fixing non-compliant files.

use crate::reader::cog_reader::{CogReader, Compression};
use std::path::Path;

/// Severity level for compliance issues
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// File will work but with degraded performance
    Warning,
    /// File will work but some features may not function correctly
    Error,
    /// File cannot be processed at all
    Critical,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Warning => write!(f, "WARNING"),
            Severity::Error => write!(f, "ERROR"),
            Severity::Critical => write!(f, "CRITICAL"),
        }
    }
}

/// A single compliance issue found during validation
#[derive(Debug, Clone)]
pub struct ComplianceIssue {
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
    pub recommendation: String,
}

/// Result of compliance check for a single file
#[derive(Debug)]
pub struct ComplianceReport {
    pub path: String,
    pub issues: Vec<ComplianceIssue>,
    pub is_compliant: bool,
    pub is_optimal: bool,
    pub details: FileDetails,
}

/// Detailed information about the file
#[derive(Debug)]
pub struct FileDetails {
    pub width: usize,
    pub height: usize,
    pub bands: usize,
    pub is_tiled: bool,
    pub tile_size: Option<(usize, usize)>,
    pub compression: String,
    pub has_overviews: bool,
    pub overview_count: usize,
    pub has_statistics: bool,
    pub has_crs: bool,
    pub has_geotransform: bool,
    pub has_nodata: bool,
    pub data_type: String,
}

impl ComplianceReport {
    #[must_use] pub fn critical_issues(&self) -> Vec<&ComplianceIssue> {
        self.issues.iter().filter(|i| i.severity == Severity::Critical).collect()
    }

    #[must_use] pub fn error_issues(&self) -> Vec<&ComplianceIssue> {
        self.issues.iter().filter(|i| i.severity == Severity::Error).collect()
    }

    #[must_use] pub fn warning_issues(&self) -> Vec<&ComplianceIssue> {
        self.issues.iter().filter(|i| i.severity == Severity::Warning).collect()
    }
}

/// Check a single file for compliance
pub fn check_file<P: AsRef<Path>>(path: P) -> Result<ComplianceReport, String> {
    let path_str = path.as_ref().to_string_lossy().to_string();

    let reader = CogReader::open(&path_str)
        .map_err(|e| format!("Failed to open file: {e}"))?;

    let metadata = &reader.metadata;
    let mut issues = Vec::new();

    // Build file details
    let details = FileDetails {
        width: metadata.width,
        height: metadata.height,
        bands: metadata.bands,
        is_tiled: metadata.is_tiled,
        tile_size: if metadata.is_tiled {
            Some((metadata.tile_width, metadata.tile_height))
        } else {
            None
        },
        compression: format!("{:?}", metadata.compression),
        has_overviews: !reader.overviews.is_empty(),
        overview_count: reader.overviews.len(),
        has_statistics: metadata.stats_min.is_some() && metadata.stats_max.is_some(),
        has_crs: metadata.crs_code.is_some(),
        has_geotransform: metadata.geo_transform.pixel_scale.is_some()
            && metadata.geo_transform.tiepoint.is_some(),
        has_nodata: metadata.nodata.is_some(),
        data_type: format!("{:?}", metadata.data_type),
    };

    // Check 1: Tiled structure (COG requirement)
    if !metadata.is_tiled {
        issues.push(ComplianceIssue {
            severity: Severity::Warning,
            code: "NOT_TILED",
            message: "File is not tiled (uses strip layout)".to_string(),
            recommendation: "Convert to tiled COG for better streaming performance. \
                Stripped files require reading entire rows, which is inefficient for tile serving."
                .to_string(),
        });
    }

    // Check 2: Tile size (should be 256x256 or 512x512)
    if metadata.is_tiled {
        let (tw, th) = (metadata.tile_width, metadata.tile_height);
        if (tw != 256 && tw != 512) || (th != 256 && th != 512) {
            issues.push(ComplianceIssue {
                severity: Severity::Warning,
                code: "NON_STANDARD_TILE_SIZE",
                message: format!("Non-standard tile size: {tw}x{th}"),
                recommendation: "Use 512x512 tiles for optimal COG performance. \
                    256x256 is also acceptable."
                    .to_string(),
            });
        }
    }

    // Check 3: Overviews (critical for large files)
    let needs_overviews = metadata.width > 1024 || metadata.height > 1024;
    if needs_overviews && reader.overviews.is_empty() {
        issues.push(ComplianceIssue {
            severity: Severity::Error,
            code: "NO_OVERVIEWS",
            message: format!(
                "Large file ({}x{}) has no overviews",
                metadata.width, metadata.height
            ),
            recommendation: "Add overviews for efficient zoom-level rendering. \
                Without overviews, tile generation at low zoom levels requires reading \
                the full resolution image."
                .to_string(),
        });
    }

    // Check 4: Statistics (min/max values)
    if metadata.stats_min.is_none() || metadata.stats_max.is_none() {
        let severity = if metadata.is_tiled && !reader.overviews.is_empty() {
            // Can estimate from smallest overview - just a warning
            Severity::Warning
        } else {
            // Will need full file scan - more serious
            Severity::Error
        };

        issues.push(ComplianceIssue {
            severity,
            code: "NO_STATISTICS",
            message: "File lacks embedded min/max statistics".to_string(),
            recommendation: "Add statistics with: gdalinfo -stats <file>. \
                Without statistics, TileYolo must scan the file to determine value range \
                for color mapping."
                .to_string(),
        });
    }

    // Check 5: CRS (coordinate reference system)
    if metadata.crs_code.is_none() {
        issues.push(ComplianceIssue {
            severity: Severity::Error,
            code: "NO_CRS",
            message: "No coordinate reference system detected".to_string(),
            recommendation: "Ensure the file has a valid CRS. TileYolo requires \
                georeferenced data to correctly position tiles."
                .to_string(),
        });
    }

    // Check 6: GeoTransform
    if metadata.geo_transform.pixel_scale.is_none() || metadata.geo_transform.tiepoint.is_none() {
        issues.push(ComplianceIssue {
            severity: Severity::Critical,
            code: "NO_GEOTRANSFORM",
            message: "Missing pixel scale or tiepoint information".to_string(),
            recommendation: "File must have valid geotransform tags (ModelPixelScaleTag \
                and ModelTiepointTag) to map pixels to geographic coordinates."
                .to_string(),
        });
    }

    // Check 7: Web Mercator (EPSG:3857) is optimal
    if let Some(crs) = metadata.crs_code
        && crs != 3857 {
            issues.push(ComplianceIssue {
                severity: Severity::Warning,
                code: "NON_WEB_MERCATOR",
                message: format!("File uses EPSG:{crs} instead of Web Mercator (EPSG:3857)"),
                recommendation: "Consider reprojecting to EPSG:3857 for optimal web tile \
                    serving. Other CRS will work but require coordinate transformation."
                    .to_string(),
            });
        }

    // Check 8: Compression (LZW or DEFLATE recommended)
    if matches!(metadata.compression, Compression::None) {
        issues.push(ComplianceIssue {
            severity: Severity::Warning,
            code: "UNCOMPRESSED",
            message: "File is uncompressed".to_string(),
            recommendation: "Use DEFLATE or LZW compression to reduce file size \
                and improve transfer speeds, especially for remote files."
                .to_string(),
        });
    }
    // Note: JPEG compression is not currently supported by this library

    // Check 9: Nodata value
    if metadata.nodata.is_none() {
        // Only a warning - many files don't need nodata
        issues.push(ComplianceIssue {
            severity: Severity::Warning,
            code: "NO_NODATA",
            message: "No nodata value defined".to_string(),
            recommendation: "Define a nodata value if the file contains invalid/missing \
                pixels. This ensures correct rendering of transparent areas."
                .to_string(),
        });
    }

    // Check 10: Sparse overview data
    // For files with overviews, check if the smallest overviews have lost too much data
    // This commonly happens with sparse data (crop yields, sparse measurements) when
    // overviews are generated with AVERAGE resampling instead of NEAREST
    if !reader.overviews.is_empty() {
        // Check the minimum usable overview that was determined during reader initialization
        if let Some(min_usable) = reader.min_usable_overview {
            let total_overviews = reader.overviews.len();
            let skipped_overviews = total_overviews.saturating_sub(min_usable + 1);

            if skipped_overviews > 0 {
                // Calculate approximate data density at the smallest usable overview
                let usable_ovr = &reader.overviews[min_usable];

                issues.push(ComplianceIssue {
                    severity: Severity::Warning,
                    code: "SPARSE_OVERVIEWS",
                    message: format!(
                        "Sparse data detected: {} of {} overviews have insufficient data. \
                        Smallest usable overview is {} ({}x{}, scale {}x).",
                        skipped_overviews,
                        total_overviews,
                        min_usable,
                        usable_ovr.width,
                        usable_ovr.height,
                        usable_ovr.scale
                    ),
                    recommendation: "For sparse data (crop yields, point measurements, etc.), \
                        regenerate the COG with NEAREST neighbor resampling to preserve data \
                        in overviews: \n\
                        gdal_translate -of COG -co COMPRESS=DEFLATE -co OVERVIEW_RESAMPLING=NEAREST \
                        <input> <output>\n\
                        This prevents AVERAGE resampling from diluting sparse values to zero."
                        .to_string(),
                });
            }
        } else {
            // No usable overview found at all - all overviews are too sparse
            if !reader.overviews.is_empty() {
                issues.push(ComplianceIssue {
                    severity: Severity::Warning,
                    code: "ALL_OVERVIEWS_SPARSE",
                    message: format!(
                        "Sparse data: all {} overviews have <5% valid pixels. \
                        Will use full resolution for best quality (slower but better visuals).",
                        reader.overviews.len()
                    ),
                    recommendation: "The source data is very sparse and all overviews have lost \
                        most data during generation. Regenerate with NEAREST neighbor resampling: \n\
                        gdal_translate -of COG -co COMPRESS=DEFLATE -co OVERVIEW_RESAMPLING=NEAREST \
                        <input> <output>\n\
                        If the issue persists, the data may be too sparse for effective overview usage."
                        .to_string(),
                });
            }
        }
    }

    // Determine overall compliance
    let has_critical = issues.iter().any(|i| i.severity == Severity::Critical);
    let has_errors = issues.iter().any(|i| i.severity == Severity::Error);
    let has_warnings = issues.iter().any(|i| i.severity == Severity::Warning);

    let is_compliant = !has_critical;
    let is_optimal = !has_critical && !has_errors && !has_warnings;

    Ok(ComplianceReport {
        path: path_str,
        issues,
        is_compliant,
        is_optimal,
        details,
    })
}

/// Recommended GDAL command for creating a compliant COG
#[must_use] pub fn recommended_gdal_command(input_file: &str, output_file: &str) -> String {
    format!(
        r#"# Create a COG with all recommended settings:
gdal_translate \
  -of COG \
  -co COMPRESS=DEFLATE \
  -co PREDICTOR=2 \
  -co BLOCKSIZE=512 \
  -co OVERVIEWS=AUTO \
  -co OVERVIEW_RESAMPLING=AVERAGE \
  "{input_file}" \
  "{output_file}"

# For SPARSE DATA (crop yields, scattered measurements, etc.) use NEAREST instead:
# gdal_translate -of COG -co COMPRESS=DEFLATE -co BLOCKSIZE=512 \
#   -co OVERVIEW_RESAMPLING=NEAREST "{input_file}" "{output_file}"
# NEAREST preserves individual data points in overviews instead of averaging to zero.

# Then add statistics:
gdalinfo -stats "{output_file}"

# Verify the result:
gdalinfo "{output_file}" | grep -E "Block=|Overviews:|STATISTICS_"
"#
    )
}

/// Short version of GDAL command
#[must_use] pub fn gdal_command_short(input_file: &str, output_file: &str) -> String {
    format!(
        "gdal_translate -of COG -co COMPRESS=DEFLATE -co BLOCKSIZE=512 \"{input_file}\" \"{output_file}\" && gdalinfo -stats \"{output_file}\""
    )
}

/// Print a compliance report to stdout
pub fn print_report(report: &ComplianceReport) {
    println!("\n{}", "=".repeat(70));
    println!("File: {}", report.path);
    println!("{}", "=".repeat(70));

    // Print file details
    println!("\nFile Details:");
    println!("  Dimensions:   {}x{} ({} bands)", report.details.width, report.details.height, report.details.bands);
    println!("  Data Type:    {}", report.details.data_type);
    println!("  Tiled:        {}", if report.details.is_tiled { "Yes" } else { "No (stripped)" });
    if let Some((tw, th)) = report.details.tile_size {
        println!("  Tile Size:    {tw}x{th}");
    }
    println!("  Compression:  {}", report.details.compression);
    println!("  Overviews:    {}", if report.details.has_overviews {
        format!("Yes ({})", report.details.overview_count)
    } else {
        "No".to_string()
    });
    println!("  Statistics:   {}", if report.details.has_statistics { "Yes" } else { "No" });
    println!("  CRS:          {}", if report.details.has_crs { "Yes" } else { "No" });
    println!("  GeoTransform: {}", if report.details.has_geotransform { "Yes" } else { "No" });
    println!("  Nodata:       {}", if report.details.has_nodata { "Yes" } else { "No" });

    // Print issues
    if report.issues.is_empty() {
        println!("\n✓ No issues found - file is fully compliant and optimized!");
    } else {
        println!("\nIssues Found ({}):", report.issues.len());
        for issue in &report.issues {
            let icon = match issue.severity {
                Severity::Critical => "✗",
                Severity::Error => "!",
                Severity::Warning => "?",
            };
            println!("\n  {} [{}] {}", icon, issue.severity, issue.code);
            println!("    {}", issue.message);
            println!("    → {}", issue.recommendation);
        }
    }

    // Print summary
    println!("\nSummary:");
    if report.is_optimal {
        println!("  ✓ File is OPTIMAL for TileYolo");
    } else if report.is_compliant {
        println!("  ~ File is COMPLIANT but not optimal");
        println!("    Consider addressing warnings for better performance.");
    } else {
        println!("  ✗ File has CRITICAL issues and may not work correctly");
    }
}

/// Summary statistics for multiple files
#[derive(Debug, Default)]
pub struct BatchSummary {
    pub total_files: usize,
    pub optimal_files: usize,
    pub compliant_files: usize,
    pub non_compliant_files: usize,
    pub failed_files: usize,
    pub issue_counts: std::collections::HashMap<&'static str, usize>,
}

impl BatchSummary {
    pub fn add_report(&mut self, report: &ComplianceReport) {
        self.total_files += 1;
        if report.is_optimal {
            self.optimal_files += 1;
        }
        if report.is_compliant {
            self.compliant_files += 1;
        } else {
            self.non_compliant_files += 1;
        }
        for issue in &report.issues {
            *self.issue_counts.entry(issue.code).or_insert(0) += 1;
        }
    }

    pub fn add_failure(&mut self) {
        self.total_files += 1;
        self.failed_files += 1;
    }

    pub fn print(&self) {
        println!("\n{}", "=".repeat(70));
        println!("BATCH SUMMARY");
        println!("{}", "=".repeat(70));
        println!("Total files:      {}", self.total_files);
        println!("  Optimal:        {} (fully compliant, no warnings)", self.optimal_files);
        println!("  Compliant:      {} (will work, may have warnings)", self.compliant_files);
        println!("  Non-compliant:  {} (have critical issues)", self.non_compliant_files);
        println!("  Failed to open: {}", self.failed_files);

        if !self.issue_counts.is_empty() {
            println!("\nIssue frequency:");
            let mut counts: Vec<_> = self.issue_counts.iter().collect();
            counts.sort_by(|a, b| b.1.cmp(a.1));
            for (code, count) in counts {
                println!("  {code}: {count} files");
            }
        }

        // Print recommended fix command if there are issues
        if self.non_compliant_files > 0 || self.optimal_files < self.compliant_files {
            println!("\nRecommended fix command for non-optimal files:");
            println!("{}", recommended_gdal_command("<input.tif>", "<output.tif>"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_compliance_check_tiled_cog() {
        let path = "data/grayscale/gray_3857-cog.tif";
        if !Path::new(path).exists() {
            println!("Skipping - file not found");
            return;
        }

        let report = check_file(path).expect("Failed to check file");
        println!("\nTiled COG report:");
        print_report(&report);

        assert!(report.is_compliant, "Tiled COG should be compliant");
        assert!(report.details.is_tiled, "Should be detected as tiled");
        assert!(report.details.has_overviews, "Should have overviews");
    }

    #[test]
    fn test_compliance_check_stripped_tiff() {
        let path = "data/test/gray_3857.tif";
        if !Path::new(path).exists() {
            println!("Skipping - file not found");
            return;
        }

        let report = check_file(path).expect("Failed to check file");
        println!("\nStripped TIFF report:");
        print_report(&report);

        // Stripped file should have NOT_TILED and NO_OVERVIEWS issues
        assert!(!report.details.is_tiled, "Should be detected as stripped");
        assert!(report.issues.iter().any(|i| i.code == "NOT_TILED"));
        assert!(report.issues.iter().any(|i| i.code == "NO_OVERVIEWS"));
    }

    #[test]
    fn test_gdal_command_generation() {
        let cmd = recommended_gdal_command("input.tif", "output_cog.tif");
        assert!(cmd.contains("gdal_translate"));
        assert!(cmd.contains("-of COG"));
        assert!(cmd.contains("COMPRESS=DEFLATE"));
        assert!(cmd.contains("gdalinfo -stats"));
    }
}
