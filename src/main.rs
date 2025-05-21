use clap::Parser;
use std::path::PathBuf;
use tileyolo::{Config, Source, TileServer};
use tokio::task;

#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Cli {
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
    #[arg(
        long,
        default_value_t = 2,
        value_name = "CACHE_SIZE_GB",
        help = "Tile cache size in GB (default: 2)"
    )]
    cache_size_gb: usize,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config = Config {
        source: Some(Source::Local(Config::parse_path_to_absolute(
            &PathBuf::from(cli.data_folder),
        ))),
        port: cli.port,
        cache_size_gb: cli.cache_size_gb,
        ..Config::default()
    };
    let server = TileServer::new(config).await?;
    server.start().await
}
