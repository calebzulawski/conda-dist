use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rattler_conda_types::ChannelConfig;

use crate::{
    cli::Cli,
    conda::{self, DEFAULT_CHANNEL, LOCKFILE_NAME},
    config, installer,
};

pub async fn execute(cli: Cli) -> Result<()> {
    let Cli {
        manifest,
        output,
        installer_platform,
    } = cli;

    let manifest_path = canonicalize_manifest(manifest)?;
    let manifest_dir = manifest_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let config = config::load_manifest(&manifest_path)?;
    let environment_name = config.name();

    let default_script_path = manifest_dir.join(format!("{environment_name}.sh"));
    let script_path =
        installer::resolve_script_path(output.unwrap_or(default_script_path), environment_name)?;

    let staging_dir = tempfile::tempdir().context("failed to create staging directory")?;
    let channel_dir = staging_dir.path().join(environment_name);
    tokio::fs::create_dir_all(&channel_dir)
        .await
        .with_context(|| {
            format!(
                "failed to create staging directory {}",
                channel_dir.display()
            )
        })?;

    let lockfile_path = channel_dir.join(LOCKFILE_NAME);

    let channel_strings = if config.channels().is_empty() {
        vec![DEFAULT_CHANNEL.to_string()]
    } else {
        config.channels().to_vec()
    };

    let channel_config = ChannelConfig::default_with_root_dir(manifest_dir.clone());
    let channels = conda::parse_channels(&channel_strings, &channel_config)?;

    let target_platforms = conda::resolve_target_platforms(config.platforms())?;
    if target_platforms.is_empty() {
        bail!("no target platforms specified");
    }

    let specs = config.dependencies().to_match_specs()?;
    if specs.is_empty() {
        bail!("no dependencies specified in manifest");
    }

    let virtual_packages = conda::detect_virtual_packages()?;
    let gateway = conda::build_gateway()?;

    let channel_urls: Vec<String> = std::iter::once("file:./".to_string())
        .chain(channels.iter().map(|ch| ch.base_url.to_string()))
        .collect();

    let solve_platforms = conda::augment_with_noarch(&target_platforms);
    let solved_records = conda::solve_environment(
        &gateway,
        &channels,
        &specs,
        &solve_platforms,
        virtual_packages,
    )
    .await?;

    let bundle_metadata = installer::PreparedBundleMetadata::from_config(
        environment_name,
        config.metadata(),
        &manifest_dir,
        &solved_records,
    )?;

    conda::download_and_index_packages(&solved_records, &channel_dir).await?;

    let lock_file = conda::build_lockfile(environment_name, &channel_urls, &solved_records)?;
    lock_file
        .to_path(&lockfile_path)
        .with_context(|| format!("failed to write lockfile to {}", lockfile_path.display()))?;

    let installer_platforms =
        installer::resolve_installer_platforms(installer_platform, &target_platforms)?;
    installer::emit_installers(
        environment_name,
        &script_path,
        &channel_dir,
        &installer_platforms,
        &bundle_metadata,
    )?;

    Ok(())
}

fn canonicalize_manifest(manifest: PathBuf) -> Result<PathBuf> {
    let display = manifest.display().to_string();
    manifest
        .canonicalize()
        .with_context(|| format!("failed to resolve manifest path {display}"))
}
