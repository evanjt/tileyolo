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
    /// Check GeoTIFF files for COG compliance and optimal configuration
    Check {
        /// Files or directories to check (defaults to ./data)
        #[arg(value_name = "PATHS")]
        paths: Vec<PathBuf>,

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
        Some(Commands::Check { paths, quiet, verbose, show_fix }) => {
            let default_path = Config::default_data_folder();
            run_compliance_check(paths, &default_path, quiet, verbose, show_fix)
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

    for file in &files_to_check {
        match compliance::check_file(file) {
            Ok(report) => {
                // Skip optimal files in quiet mode
                if quiet && report.is_optimal {
                    summary.add_report(&report);
                    continue;
                }

                if verbose {
                    compliance::print_report(&report);
                } else {
                    // Brief output
                    let status = if report.is_optimal {
                        "✓ OPTIMAL"
                    } else if report.is_compliant {
                        "~ COMPLIANT"
                    } else {
                        "✗ ISSUES"
                    };

                    let issues_str = if report.issues.is_empty() {
                        String::new()
                    } else {
                        let codes: Vec<_> = report.issues.iter().map(|i| i.code).collect();
                        format!(" [{}]", codes.join(", "))
                    };

                    println!("{} {}{}", status, file.display(), issues_str);
                }

                if show_fix && !report.is_optimal {
                    let input = file.to_string_lossy();
                    let output = format!("{}_cog.tif",
                        file.file_stem().map(|s| s.to_string_lossy()).unwrap_or_default());
                    println!("  Fix: {}", compliance::gdal_command_short(&input, &output));
                }

                summary.add_report(&report);
            }
            Err(e) => {
                println!("✗ FAILED {} - {}", file.display(), e);
                summary.add_failure();
            }
        }
    }

    // Print summary
    summary.print();

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
        .map(|ext| {
            let ext = ext.to_string_lossy().to_lowercase();
            ext == "tif" || ext == "tiff"
        })
        .unwrap_or(false)
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
