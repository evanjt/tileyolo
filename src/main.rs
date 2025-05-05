use clap::Parser;
use std::path::PathBuf;
use tileyolo::{Config, Source, TileServer};

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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Use the value from the command line argument (default from config is provided to it already)
    let config = Config {
        source: Some(Source::Local(Config::parse_path_to_absolute(
            &PathBuf::from(cli.data_folder),
        ))),
        port: cli.port,
        ..Config::default() // Then fill with the rest of the default config
    };

    let server = TileServer::new(config)?;

    server.start().await
}
