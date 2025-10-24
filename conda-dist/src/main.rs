mod app;
mod cli;
mod conda;
mod config;
mod container;
mod downloader;
mod installer;
mod progress;
mod workspace;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = cli::parse();
    app::execute(cli).await
}
