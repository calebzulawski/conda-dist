use std::{path::PathBuf, time::Duration};

use anyhow::{Result, bail};

use crate::{cli::InstallerArgs, conda, installer, progress::Progress, workspace::Workspace};

use super::{context::load_manifest_context, environment::prepare_environment};

pub async fn execute(args: InstallerArgs, work_dir: Option<PathBuf>) -> Result<()> {
    let InstallerArgs {
        manifest,
        output,
        installer_platform,
        unlock,
    } = args;

    let manifest_ctx = load_manifest_context(manifest)?;
    let environment_name = manifest_ctx.config.name();
    let workspace = Workspace::from_manifest_dir(&manifest_ctx.manifest_dir, work_dir)?;

    let default_script_path = manifest_ctx.manifest_dir.join(environment_name);
    let script_path =
        installer::resolve_script_path(output.unwrap_or(default_script_path), environment_name)?;

    let target_platforms = conda::resolve_target_platforms(manifest_ctx.config.platforms())?;
    if target_platforms.is_empty() {
        bail!("no target platforms specified");
    }

    let progress = Progress::stdout();
    let mut final_messages = Vec::new();

    let (prep, download_summary) = prepare_environment(
        &manifest_ctx,
        &workspace,
        target_platforms,
        unlock,
        &progress,
    )
    .await?;

    let installer_platforms =
        installer::resolve_installer_platforms(installer_platform, &prep.target_platforms)?;

    let total_installers = installer_platforms.len();
    let installer_step = progress.step("Create installers");
    let script_path_ref = &script_path;
    let prep_ref = &prep;
    let installer_platforms_ref = &installer_platforms;
    let written_paths = installer_step
        .run_with(
            Some(Duration::from_millis(120)),
            move |handle| async move {
                let mut counter = handle.counter(total_installers);
                installer::create_installers(
                    script_path_ref,
                    &prep_ref.environment_name,
                    &prep_ref.channel_dir,
                    installer_platforms_ref,
                    &prep_ref.bundle_metadata,
                    &mut counter,
                )
            },
            move |paths| format!("Create installers ({}/{total_installers})", paths.len()),
        )
        .await?;

    if download_summary.fetched_packages == 0 {
        final_messages.push("No packages required downloading.".to_string());
    } else {
        let reused = download_summary
            .total_packages
            .saturating_sub(download_summary.fetched_packages);
        if reused > 0 {
            final_messages.push(format!(
                "Downloaded {} packages (reused {}).",
                download_summary.fetched_packages, reused
            ));
        } else {
            final_messages.push(format!(
                "Downloaded {} packages.",
                download_summary.fetched_packages
            ));
        }
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
