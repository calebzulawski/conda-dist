use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use rattler_conda_types::{ChannelConfig, Platform};
use tempfile::TempDir;

use crate::{
    cli::{Cli, Command, InstallerArgs},
    conda::{self, DEFAULT_CHANNEL, LOCKFILE_NAME},
    config, container, installer,
    progress::Progress,
};

pub async fn execute(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Installer(args) => execute_installer(args).await,
        Command::Container(args) => container::execute(args).await,
    }
}

async fn execute_installer(args: InstallerArgs) -> Result<()> {
    let InstallerArgs {
        manifest,
        output,
        installer_platform,
    } = args;

    let manifest_ctx = load_manifest_context(manifest)?;
    let environment_name = manifest_ctx.config.name();

    let default_script_path = manifest_ctx
        .manifest_dir
        .join(format!("{environment_name}.sh"));
    let script_path =
        installer::resolve_script_path(output.unwrap_or(default_script_path), environment_name)?;

    let target_platforms = conda::resolve_target_platforms(manifest_ctx.config.platforms())?;
    if target_platforms.is_empty() {
        bail!("no target platforms specified");
    }

    let progress = Progress::stdout();
    let mut final_messages = Vec::new();

    let (prep, downloaded_count) =
        prepare_environment(&manifest_ctx, target_platforms.clone(), &progress).await?;

    let installer_platforms =
        installer::resolve_installer_platforms(installer_platform, &prep.target_platforms)?;

    let total_installers = installer_platforms.len();
    let installer_step = progress.step("Create installers");
    let installer_bar = installer_step.clone_bar();
    let script_path_ref = &script_path;
    let prep_ref = &prep;
    let installer_platforms_ref = &installer_platforms;
    let written_paths = installer_step
        .run(
            Some(Duration::from_millis(120)),
            async move {
                installer::create_installers(
                    script_path_ref,
                    &prep_ref.environment_name,
                    &prep_ref.channel_dir,
                    installer_platforms_ref,
                    &prep_ref.bundle_metadata,
                    &installer_bar,
                )
            },
            move |paths| format!("Create installers ({}/{total_installers})", paths.len()),
        )
        .await?;

    if downloaded_count == 0 {
        final_messages.push("No packages required downloading.".to_string());
    }

    if !written_paths.is_empty() {
        final_messages.push("Installer outputs:".to_string());
        for path in written_paths {
            final_messages.push(format!("  - {}", path.display()));
        }
    }

    drop(progress);

    for message in final_messages {
        println!("{}", message);
    }

    Ok(())
}

#[derive(Debug)]
pub struct ManifestContext {
    pub manifest_dir: PathBuf,
    pub config: config::CondaDistConfig,
}

#[derive(Debug)]
pub struct EnvironmentPreparation {
    pub environment_name: String,
    pub staging_dir: TempDir,
    pub channel_dir: PathBuf,
    pub bundle_metadata: installer::PreparedBundleMetadata,
    pub target_platforms: Vec<Platform>,
}

pub(crate) fn canonicalize_manifest(manifest: PathBuf) -> Result<PathBuf> {
    let display = manifest.display().to_string();
    manifest
        .canonicalize()
        .with_context(|| format!("failed to resolve manifest path {display}"))
}

pub(crate) fn load_manifest_context(manifest: PathBuf) -> Result<ManifestContext> {
    let manifest_path = canonicalize_manifest(manifest)?;
    let manifest_dir = manifest_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let config = config::load_manifest(&manifest_path)?;

    Ok(ManifestContext {
        manifest_dir,
        config,
    })
}

pub(crate) async fn prepare_environment(
    manifest_ctx: &ManifestContext,
    target_platforms: Vec<Platform>,
    progress: &Progress,
) -> Result<(EnvironmentPreparation, usize)> {
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

    let solve_step = progress.step("Solve environment");
    let solve_platforms = conda::augment_with_noarch(&target_platforms);
    let solved_records = solve_step
        .run(
            Some(Duration::from_millis(120)),
            conda::solve_environment(
                &gateway,
                &channels,
                &specs,
                &solve_platforms,
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

    let download_step = progress.step("Download packages");
    let download_bar = download_step.clone_bar();
    let downloaded_count = download_step
        .run(
            None,
            conda::download_and_index_packages(&solved_records, &channel_dir, &download_bar),
            |count| format!("Download packages ({count}/{count})"),
        )
        .await?;

    let lockfile_path = channel_dir.join(LOCKFILE_NAME);
    let lock_file = conda::build_lockfile(&environment_name, &channel_urls, &solved_records)?;
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

    Ok((preparation, downloaded_count))
}
