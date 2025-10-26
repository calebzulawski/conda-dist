use std::{path::PathBuf, time::Duration};

use anyhow::{Context, Result, bail};
use rattler_conda_types::{ChannelConfig, Platform};
use tempfile::TempDir;

use crate::{
    conda::{self, DEFAULT_CHANNEL, LOCKFILE_NAME},
    downloader, installer,
    progress::Progress,
    workspace::Workspace,
};

use super::context::ManifestContext;

#[derive(Debug)]
pub struct EnvironmentPreparation {
    pub environment_name: String,
    pub staging_dir: TempDir,
    pub channel_dir: PathBuf,
    pub bundle_metadata: installer::PreparedBundleMetadata,
    pub target_platforms: Vec<Platform>,
}

pub async fn prepare_environment(
    manifest_ctx: &ManifestContext,
    workspace: &Workspace,
    target_platforms: Vec<Platform>,
    unlock_lockfile: bool,
    progress: &Progress,
) -> Result<(EnvironmentPreparation, downloader::DownloadSummary)> {
    let environment_name = manifest_ctx.config.name().to_string();

    let staging_dir = tempfile::tempdir().context("failed to create staging directory")?;
    let channel_dir = staging_dir.path().join(&environment_name);
    tokio::fs::create_dir_all(&channel_dir)
        .await
        .with_context(|| {
            format!(
                "failed to create staging directory {}",
                channel_dir.display()
            )
        })?;

    let channel_strings = if manifest_ctx.config.channels().is_empty() {
        vec![DEFAULT_CHANNEL.to_string()]
    } else {
        manifest_ctx.config.channels().to_vec()
    };

    let channel_config = ChannelConfig::default_with_root_dir(manifest_ctx.manifest_dir.clone());
    let channels = conda::parse_channels(&channel_strings, &channel_config)?;

    let specs = manifest_ctx.config.dependencies().to_match_specs()?;
    if specs.is_empty() {
        bail!("no dependencies specified in manifest");
    }

    let virtual_packages = conda::detect_virtual_packages()?;
    let gateway = conda::build_gateway()?;

    let channel_urls: Vec<String> = std::iter::once("file:./".to_string())
        .chain(channels.iter().map(|ch| ch.base_url.to_string()))
        .collect();

    let solve_platforms = conda::augment_with_noarch(&target_platforms);
    let workspace_lockfile_path = workspace.lockfile_path();
    let locked_packages = if !unlock_lockfile && workspace_lockfile_path.exists() {
        conda::load_locked_packages(
            &workspace_lockfile_path,
            &environment_name,
            &solve_platforms,
        )?
    } else {
        Vec::new()
    };

    let solve_step = progress.step("Solve environment");
    let solved_records = solve_step
        .run(
            Some(Duration::from_millis(120)),
            conda::solve_environment(
                &gateway,
                &channels,
                &specs,
                &solve_platforms,
                locked_packages,
                virtual_packages,
            ),
            |_| "Solve environment".to_string(),
        )
        .await?;

    let bundle_metadata = installer::PreparedBundleMetadata::from_config(
        &environment_name,
        manifest_ctx.config.metadata(),
        &manifest_ctx.manifest_dir,
        &solved_records,
    )?;

    let package_cache_dir = workspace.package_cache_dir();

    let download_step = progress.step("Download packages");
    let download_bar = download_step.clone_bar();
    let download_summary = download_step
        .run(
            None,
            downloader::download_and_stage_packages(
                &solved_records,
                &channel_dir,
                &package_cache_dir,
                &download_bar,
            ),
            |summary| {
                format!(
                    "Download packages ({}/{})",
                    summary.total_packages, summary.total_packages
                )
            },
        )
        .await?;

    let lock_file = conda::build_lockfile(&environment_name, &channel_urls, &solved_records)?;
    lock_file
        .to_path(&workspace_lockfile_path)
        .with_context(|| {
            format!(
                "failed to write lockfile to workspace at {}",
                workspace_lockfile_path.display()
            )
        })?;

    let lockfile_path = channel_dir.join(LOCKFILE_NAME);
    lock_file
        .to_path(&lockfile_path)
        .with_context(|| format!("failed to write lockfile to {}", lockfile_path.display()))?;

    let preparation = EnvironmentPreparation {
        environment_name,
        staging_dir,
        channel_dir,
        bundle_metadata,
        target_platforms,
    };

    Ok((preparation, download_summary))
}
