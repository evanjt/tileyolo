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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // this calls Config::default_data_folder() once under the hood,
    // uses it for the help text, and again as the default if the user
    // doesn’t specify --data-folder
    let cli = Cli::parse();

    // start from your actual Config::default()
    let config = Config {
        source: Some(Source::Local(Config::parse_path_to_absolute(
            &PathBuf::from(cli.data_folder),
        ))),
        ..Config::default()
    };
    // println!("📦 Config: {:#?}", config);

    let server = TileServer::new(config)?;
    server.start().await
}
