mod bundle;
mod cli;
mod listing;
mod operations;

use std::env;

use anyhow::{Result, anyhow};

#[tokio::main]
async fn main() -> Result<()> {
    let launcher_metadata = bundle::read_embedded_metadata()?;
    let project_name = env::var("CONDA_DIST_PROJECT_NAME")
        .unwrap_or_else(|_| launcher_metadata.display_name.clone());

    let cli = cli::parse(&project_name)?;

    if cli.summary {
        let bundle = bundle::load_bundle()?;
        listing::print_bundle_summary(&bundle.metadata, &bundle.records, &bundle.channel_dir)?;
        return Ok(());
    }

    if cli.list_packages_json {
        let bundle = bundle::load_bundle()?;
        listing::list_packages_json(&bundle.records)?;
        return Ok(());
    }

    if cli.list_packages {
        let bundle = bundle::load_bundle()?;
        listing::list_packages_plain(&bundle.records);
        return Ok(());
    }

    let prefix = cli.prefix.ok_or_else(|| {
        anyhow!(
            "installation path is required unless --summary/--list-packages/--list-packages-json is used"
        )
    })?;

    let bundle = bundle::load_bundle()?;
    operations::install(&prefix, &bundle).await
}
