use std::{
    future::Future,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use rattler_conda_types::{ChannelConfig, Platform};
use tempfile::TempDir;

use crate::{
    cli::{Cli, Command, InstallerArgs},
    conda::{self, DEFAULT_CHANNEL, LOCKFILE_NAME},
    config, container, installer,
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

    let multi = MultiProgress::with_draw_target(ProgressDrawTarget::stdout());
    let step_style = ProgressStyle::with_template("{prefix} {msg}")
        .unwrap_or_else(|_| ProgressStyle::default_spinner());

    let (prep, downloaded_count) =
        prepare_environment(&manifest_ctx, target_platforms.clone(), &multi, &step_style).await?;

    let installer_step = progress_step(&multi, &step_style, "Create installers");
    let installer_platforms =
        installer::resolve_installer_platforms(installer_platform, &prep.target_platforms)?;

    let total_installers = installer_platforms.len();
    let written_paths = run_step(
        &installer_step,
        "Create installers",
        Some(Duration::from_millis(120)),
        async {
            installer::create_installers(
                &script_path,
                &prep.environment_name,
                &prep.channel_dir,
                &installer_platforms,
                &prep.bundle_metadata,
                &installer_step,
            )
        },
        move |paths| format!("Create installers ({}/{total_installers})", paths.len()),
    )
    .await?;

    if downloaded_count == 0 {
        let _ = multi.println("No packages required downloading.");
    }

    if !written_paths.is_empty() {
        let _ = multi.println("Installer outputs:");
        for path in written_paths {
            let _ = multi.println(format!("  - {}", path.display()));
        }
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
    multi: &MultiProgress,
    step_style: &ProgressStyle,
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

    let solve_step = progress_step(multi, step_style, "Solve environment");
    let download_step = progress_step(multi, step_style, "Download packages");

    let solve_platforms = conda::augment_with_noarch(&target_platforms);
    let solved_records = run_step(
        &solve_step,
        "Solve environment",
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

    let downloaded_count = run_step(
        &download_step,
        "Download packages",
        None,
        conda::download_and_index_packages(&solved_records, &channel_dir, &download_step),
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

fn progress_step(multi: &MultiProgress, style: &ProgressStyle, label: &str) -> ProgressBar {
    let step = multi.add(ProgressBar::new_spinner());
    step.set_style(style.clone());
    step.set_prefix("[ ]");
    step.set_message(label.to_string());
    step.tick();
    step
}

async fn run_step<F, T, S>(
    step: &ProgressBar,
    label: &str,
    steady_tick: Option<Duration>,
    future: F,
    success_message: S,
) -> Result<T>
where
    F: Future<Output = Result<T>>,
    S: FnOnce(&T) -> String,
{
    step.set_prefix("[…]");
    if let Some(interval) = steady_tick {
        step.enable_steady_tick(interval);
    }

    match future.await {
        Ok(value) => {
            step.set_prefix("[✔]");
            let message = success_message(&value);
            step.finish_with_message(message);
            Ok(value)
        }
        Err(err) => {
            step.set_prefix("[✖]");
            step.finish_with_message(format!("{label} (failed)"));
            Err(err)
        }
    }
}
