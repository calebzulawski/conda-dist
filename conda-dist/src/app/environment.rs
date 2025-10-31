use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use rattler_conda_types::{ChannelConfig, Platform, RepoDataRecord};
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

    let gateway = conda::build_gateway()?;

    let channel_urls: Vec<String> = std::iter::once("file:./".to_string())
        .chain(channels.iter().map(|ch| ch.base_url.to_string()))
        .collect();

    let workspace_lockfile_path = workspace.lockfile_path();
    let solve_platforms_for_lock = conda::augment_with_noarch(&target_platforms);
    let locked_packages_all = if !unlock_lockfile && workspace_lockfile_path.exists() {
        conda::load_locked_packages(
            &workspace_lockfile_path,
            &environment_name,
            &solve_platforms_for_lock,
        )?
    } else {
        Vec::new()
    };

    let mut locked_by_subdir: HashMap<String, Vec<RepoDataRecord>> = HashMap::new();
    for record in locked_packages_all {
        locked_by_subdir
            .entry(record.package_record.subdir.clone())
            .or_default()
            .push(record);
    }

    let virtual_package_config = manifest_ctx.config.virtual_packages();
    let total_platforms = target_platforms.len();
    let solve_step = progress.step("Solve environment");
    let solve_bar = solve_step.clone_bar();
    let target_platforms_for_solve = target_platforms.clone();
    let channels_for_solve = channels;
    let specs_for_solve = specs;
    let locked_by_subdir_for_solve = locked_by_subdir;
    let solved_records = solve_step
        .run(
            Some(Duration::from_millis(120)),
            async move {
                solve_bar.set_message(format!("Solve environment (0/{total_platforms})"));
                solve_bar.tick();

                let mut combined = Vec::new();
                let mut seen: HashSet<(String, String)> = HashSet::new();
                for (index, platform) in target_platforms_for_solve.iter().enumerate() {
                    let solve_platforms = conda::augment_with_noarch(&[*platform]);
                    let mut locked_for_platform = locked_by_subdir_for_solve
                        .get(platform.as_str())
                        .cloned()
                        .unwrap_or_default();
                    if let Some(noarch_locked) =
                        locked_by_subdir_for_solve.get(Platform::NoArch.as_str())
                    {
                        locked_for_platform.extend(noarch_locked.iter().cloned());
                    }

                    let overrides =
                        virtual_package_config.and_then(|cfg| cfg.for_platform(*platform));
                    let virtual_packages =
                        conda::detect_virtual_packages_for_platform(*platform, overrides)?;
                    let records = conda::solve_environment(
                        &gateway,
                        &channels_for_solve,
                        &specs_for_solve,
                        &solve_platforms,
                        locked_for_platform,
                        virtual_packages,
                    )
                    .await
                    .with_context(|| {
                        format!(
                            "failed to solve environment for platform {}",
                            platform.as_str()
                        )
                    })?;

                    for record in records {
                        let key = (
                            record.package_record.subdir.clone(),
                            record.file_name.clone(),
                        );
                        if seen.insert(key) {
                            combined.push(record);
                        }
                    }

                    let done = index + 1;
                    solve_bar.set_message(format!("Solve environment ({done}/{total_platforms})"));
                    solve_bar.tick();
                }

                Ok(combined)
            },
            |_| format!("Solve environment ({total_platforms}/{total_platforms})"),
        )
        .await?;

    let bundle_metadata = installer::PreparedBundleMetadata::from_config(
        &environment_name,
        manifest_ctx.config.metadata(),
        &manifest_ctx.manifest_dir,
        &solved_records,
        manifest_ctx.config.author(),
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
