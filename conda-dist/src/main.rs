use anyhow::Result;
use conda_dist::{app, cli};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = cli::parse();
    app::execute(cli).await
}
