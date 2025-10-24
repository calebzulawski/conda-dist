mod app;
mod cli;
mod conda;
mod config;
mod container;
mod installer;
mod progress;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = cli::parse();
    app::execute(cli).await
}
