use tileyolo::{Config, TileServer};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::default();
    let server = TileServer::new(config)?;
    server.start().await
}
