mod bundle;
mod cli;
mod listing;
mod operations;

use std::env;

use anyhow::{Result, anyhow};

#[tokio::main]
async fn main() -> Result<()> {
    let project_name =
        env::var("CONDA_DIST_PROJECT_NAME").unwrap_or_else(|_| "this project".to_string());

    let cli = cli::parse(&project_name)?;
    let bundle = bundle::load_bundle()?;

    if cli.summary {
        listing::print_bundle_summary(&bundle.metadata, &bundle.records, &bundle.channel_dir)?;
        return Ok(());
    }

    if cli.list_packages_json {
        listing::list_packages_json(&bundle.records)?;
        return Ok(());
    }

    if cli.list_packages {
        listing::list_packages_plain(&bundle.records);
        return Ok(());
    }

    let prefix = cli.prefix.ok_or_else(|| {
        anyhow!(
            "installation path is required unless --summary/--list-packages/--list-packages-json is used"
        )
    })?;

    operations::install(&prefix, &bundle, &project_name).await
}
