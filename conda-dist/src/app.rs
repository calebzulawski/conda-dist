use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
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

    let multi = MultiProgress::with_draw_target(ProgressDrawTarget::stdout());
    let step_style = ProgressStyle::with_template("{prefix} {msg}")
        .unwrap_or_else(|_| ProgressStyle::default_spinner());

    let solve_step = multi.add(ProgressBar::new_spinner());
    solve_step.set_style(step_style.clone());
    solve_step.set_prefix("[ ]");
    solve_step.set_message("Solve environment");
    solve_step.tick();

    let download_step = multi.add(ProgressBar::new_spinner());
    download_step.set_style(step_style.clone());
    download_step.set_prefix("[ ]");
    download_step.set_message("Download packages");
    download_step.tick();

    let installer_step = multi.add(ProgressBar::new_spinner());
    installer_step.set_style(step_style.clone());
    installer_step.set_prefix("[ ]");
    installer_step.set_message("Create installers");
    installer_step.tick();

    let solve_platforms = conda::augment_with_noarch(&target_platforms);
    solve_step.set_prefix("[…]");
    solve_step.enable_steady_tick(Duration::from_millis(120));
    let solved_records = match conda::solve_environment(
        &gateway,
        &channels,
        &specs,
        &solve_platforms,
        virtual_packages,
    )
    .await
    {
        Ok(records) => {
            solve_step.set_prefix("[✔]");
            solve_step.finish_with_message("Solve environment");
            records
        }
        Err(err) => {
            solve_step.set_prefix("[✖]");
            solve_step.finish_with_message("Solve environment (failed)");
            return Err(err);
        }
    };

    let bundle_metadata = installer::PreparedBundleMetadata::from_config(
        environment_name,
        config.metadata(),
        &manifest_dir,
        &solved_records,
    )?;

    download_step.set_prefix("[…]");
    let downloaded_count =
        match conda::download_and_index_packages(&solved_records, &channel_dir, &download_step)
            .await
        {
            Ok(count) => {
                download_step.set_prefix("[✔]");
                download_step.finish_with_message(format!("Download packages ({count}/{count})"));
                count
            }
            Err(err) => {
                download_step.set_prefix("[✖]");
                download_step.finish_with_message("Download packages (failed)");
                return Err(err);
            }
        };

    let lock_file = conda::build_lockfile(environment_name, &channel_urls, &solved_records)?;
    lock_file
        .to_path(&lockfile_path)
        .with_context(|| format!("failed to write lockfile to {}", lockfile_path.display()))?;

    let installer_platforms =
        installer::resolve_installer_platforms(installer_platform, &target_platforms)?;

    installer_step.set_prefix("[…]");
    installer_step.enable_steady_tick(Duration::from_millis(120));
    let written_paths = match installer::create_installers(
        &script_path,
        environment_name,
        &channel_dir,
        &installer_platforms,
        &bundle_metadata,
        &installer_step,
    ) {
        Ok(paths) => {
            installer_step.set_prefix("[✔]");
            installer_step.finish_with_message(format!(
                "Create installers ({}/{})",
                paths.len(),
                installer_platforms.len()
            ));
            paths
        }
        Err(err) => {
            installer_step.set_prefix("[✖]");
            installer_step.finish_with_message("Create installers (failed)");
            return Err(err);
        }
    };

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

fn canonicalize_manifest(manifest: PathBuf) -> Result<PathBuf> {
    let display = manifest.display().to_string();
    manifest
        .canonicalize()
        .with_context(|| format!("failed to resolve manifest path {display}"))
}
