use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tileyolo::{Config, Source, TileServer};
use tileyolo::reader::compliance::{self, BatchSummary};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Check `GeoTIFF` files for COG compliance and optimal configuration
    Check {
        /// Files or directories to check (defaults to --data-folder)
        #[arg(value_name = "PATHS")]
        paths: Vec<PathBuf>,

        /// Path to the data folder (used when no PATHS specified)
        #[arg(
            long,
            default_value_t = Config::default_data_folder(),
            value_name = "DATA_FOLDER",
            help = "Path to the data folder"
        )]
        data_folder: String,

        /// Show only files with issues (hide optimal files)
        #[arg(long, short = 'q')]
        quiet: bool,

        /// Show detailed recommendations for fixing issues
        #[arg(long, short = 'v')]
        verbose: bool,

        /// Show GDAL command to fix non-compliant files
        #[arg(long)]
        show_fix: bool,
    },

    /// Start the tile server (default if no subcommand given)
    Serve {
        /// Where tiles and assets live
        #[arg(
            long,
            default_value_t = Config::default_data_folder(),
            value_name = "DATA_FOLDER",
            help = "Path to the data folder"
        )]
        data_folder: String,

        #[arg(
            long,
            default_value_t = Config::default_port(),
            value_name = "PORT",
            help = "Port to run the server on"
        )]
        port: u16,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Check { paths, data_folder, quiet, verbose, show_fix }) => {
            run_compliance_check(paths, &data_folder, quiet, verbose, show_fix)
        }
        Some(Commands::Serve { data_folder, port }) => {
            run_server(&data_folder, port).await
        }
        None => {
            // Default: serve with default settings
            run_server(&Config::default_data_folder(), Config::default_port()).await
        }
    }
}

fn run_compliance_check(
    paths: Vec<PathBuf>,
    data_folder: &str,
    quiet: bool,
    verbose: bool,
    show_fix: bool,
) -> anyhow::Result<()> {
    // Collect files to check
    let files_to_check = if paths.is_empty() {
        // Use data folder
        let data_path = Config::parse_path_to_absolute(&PathBuf::from(data_folder));
        collect_tiff_files(&data_path)?
    } else {
        // Use provided paths
        let mut files = Vec::new();
        for path in paths {
            if path.is_dir() {
                files.extend(collect_tiff_files(&path)?);
            } else if is_tiff_file(&path) {
                files.push(path);
            }
        }
        files
    };

    if files_to_check.is_empty() {
        println!("No GeoTIFF files found to check.");
        return Ok(());
    }

    println!("Checking {} file(s) for COG compliance...\n", files_to_check.len());

    let mut summary = BatchSummary::default();
    let mut results: Vec<(String, String, Vec<String>)> = Vec::new(); // (filename, status, issues)
    let mut failed: Vec<(String, String)> = Vec::new(); // (filename, error)

    for file in &files_to_check {
        let filename = file.file_name().map_or_else(|| file.display().to_string(), |n| n.to_string_lossy().to_string());

        match compliance::check_file(file) {
            Ok(report) => {
                if verbose {
                    compliance::print_report(&report);
                }

                let status = if report.is_optimal {
                    "✓ OK"
                } else if report.is_compliant {
                    "~ WARN"
                } else {
                    "✗ ERR"
                };

                let issues: Vec<String> = report.issues.iter().map(|i| i.code.to_string()).collect();
                results.push((filename, status.to_string(), issues));
                summary.add_report(&report);
            }
            Err(e) => {
                failed.push((filename, e.to_string()));
                summary.add_failure();
            }
        }
    }

    // Print table of results
    if !verbose {
        println!("┌─────────────────────────────────────────┬────────┬─────────────────────────────────────┐");
        println!("│ File                                    │ Status │ Issues                              │");
        println!("├─────────────────────────────────────────┼────────┼─────────────────────────────────────┤");

        for (filename, status, issues) in &results {
            if quiet && status == "✓ OK" {
                continue;
            }
            let short_name = if filename.len() > 39 {
                format!("...{}", &filename[filename.len()-36..])
            } else {
                filename.clone()
            };
            let issues_str = if issues.is_empty() {
                "-".to_string()
            } else {
                issues.join(", ")
            };
            let short_issues = if issues_str.len() > 35 {
                format!("{}...", &issues_str[..32])
            } else {
                issues_str
            };
            println!("│ {short_name:<39} │ {status:<6} │ {short_issues:<35} │");
        }

        // Print failed files
        for (filename, error) in &failed {
            let short_name = if filename.len() > 39 {
                format!("...{}", &filename[filename.len()-36..])
            } else {
                filename.clone()
            };
            let short_err = if error.len() > 35 {
                format!("{}...", &error[..32])
            } else {
                error.clone()
            };
            println!("│ {short_name:<39} │ ✗ FAIL │ {short_err:<35} │");
        }

        println!("└─────────────────────────────────────────┴────────┴─────────────────────────────────────┘");
    }

    // Print fix commands section if requested or if there are issues
    let needs_fixing: Vec<_> = files_to_check.iter()
        .filter(|f| {
            let fname = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            results.iter().any(|(n, s, _)| n == &fname && s != "✓ OK") ||
            failed.iter().any(|(n, _)| n == &fname)
        })
        .collect();

    if show_fix && !needs_fixing.is_empty() {
        println!("\n# Commands to convert files to compliant COG format:");
        println!("# (Copy and run these in your terminal)\n");
        println!("mkdir -p ./cog_output\n");
        for file in needs_fixing {
            let input = file.to_string_lossy();
            let filename = file.file_name().map_or_else(|| "output.tif".to_string(), |s| s.to_string_lossy().to_string());
            // Use OVERVIEW_RESAMPLING=NEAREST to preserve sparse data in overviews
            println!("gdal_translate -of COG -co COMPRESS=DEFLATE -co BLOCKSIZE=512 -co OVERVIEW_RESAMPLING=NEAREST \"{input}\" \"./cog_output/{filename}\"");
        }
        println!("\n# After conversion, add statistics to all files:");
        println!("for f in ./cog_output/*.tif; do gdalinfo -stats \"$f\"; done");
        println!("\n# Note: OVERVIEW_RESAMPLING=NEAREST preserves sparse data at low zoom levels.");
        println!("# For continuous data (like elevation), use OVERVIEW_RESAMPLING=AVERAGE instead.");
    }

    // Print summary
    println!("\nSummary: {} total, {} optimal, {} warnings, {} failed",
        summary.total_files,
        summary.optimal_files,
        summary.compliant_files - summary.optimal_files,
        summary.failed_files + summary.non_compliant_files
    );

    if !show_fix && (summary.failed_files > 0 || summary.compliant_files < summary.total_files) {
        println!("\nRun with --show-fix to get GDAL commands for converting files.");
    }

    // Exit with error code if there are non-compliant files
    if summary.non_compliant_files > 0 || summary.failed_files > 0 {
        std::process::exit(1);
    }

    Ok(())
}

fn collect_tiff_files(dir: &PathBuf) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();

    if !dir.exists() {
        return Err(anyhow::anyhow!("Directory does not exist: {}", dir.display()));
    }

    if dir.is_file() {
        if is_tiff_file(dir) {
            files.push(dir.clone());
        }
        return Ok(files);
    }

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            // Recursively check subdirectories
            files.extend(collect_tiff_files(&path)?);
        } else if is_tiff_file(&path) {
            files.push(path);
        }
    }

    Ok(files)
}

fn is_tiff_file(path: &PathBuf) -> bool {
    path.extension()
        .is_some_and(|ext| {
            let ext = ext.to_string_lossy().to_lowercase();
            ext == "tif" || ext == "tiff"
        })
}

async fn run_server(data_folder: &str, port: u16) -> anyhow::Result<()> {
    let config = Config {
        source: Some(Source::Local(Config::parse_path_to_absolute(
            &PathBuf::from(data_folder),
        ))),
        port,
        ..Config::default()
    };

    let server = TileServer::new(config).await?;
    server.start().await
}
