use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Result, bail};
use rattler_conda_types::Platform;

use crate::{
    cli::InstallerArgs, config::CondaDistConfig, progress::Progress, workspace::Workspace,
};

use super::{
    LockMode,
    context::load_manifest_context,
    environment::{EnvironmentPreparation, prepare_environment},
};

mod bundle;

pub use bundle::BundleMetadataManifest;
use bundle::create_installers;

pub(super) type InstallerPaths = BTreeMap<String, BTreeMap<Platform, PathBuf>>;

pub async fn execute(
    args: InstallerArgs,
    work_dir: Option<PathBuf>,
    lock_mode: LockMode,
) -> Result<()> {
    let InstallerArgs {
        manifest,
        output_dir,
        installer_platform,
    } = args;

    let manifest_ctx = load_manifest_context(manifest)?;
    let workspace = Workspace::from_manifest_dir(&manifest_ctx.manifest_dir, work_dir)?;

    let installer_platforms: Vec<_> = manifest_ctx
        .config
        .platforms()
        .iter()
        .copied()
        .filter(|platform| *platform != Platform::NoArch && installer_platform.contains(platform))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if installer_platforms.is_empty() {
        bail!("no installer platforms selected from manifest");
    }

    let progress = Progress::stdout();

    let prep = prepare_environment(
        &manifest_ctx,
        &workspace,
        installer_platforms.clone(),
        lock_mode,
        &progress,
    )
    .await?;

    let output_dir = resolve_output_dir(output_dir, &manifest_ctx.manifest_dir)?;
    let total_installers = installer_platforms.len() * (manifest_ctx.config.subsets().len() + 1);
    let installer_step = progress.step("Create installers");
    let output_dir_ref = &output_dir;
    let prep_ref = &prep;
    let config = &manifest_ctx.config;
    let written_paths = installer_step
        .run_with(
            Some(Duration::from_millis(120)),
            move |handle| async move {
                let mut counter = handle.counter(total_installers);
                create_installers(output_dir_ref, config, prep_ref, &mut counter)
            },
            move |_| format!("Create installers ({total_installers}/{total_installers})"),
        )
        .await?;

    drop(progress);
    if !written_paths.is_empty() {
        println!("Installer outputs:");
        for path in written_paths.into_values().flat_map(BTreeMap::into_values) {
            println!("  - {}", path.display());
        }
    }

    Ok(())
}

pub(super) async fn prepare_installers(
    prep: &EnvironmentPreparation,
    config: &CondaDistConfig,
    output_dir: &Path,
    progress: &Progress,
) -> Result<InstallerPaths> {
    let count = prep.target_platforms.len() * (config.subsets().len() + 1);
    progress
        .step("Prepare installer bundles")
        .run_with(
            Some(Duration::from_millis(120)),
            |handle| async move {
                let mut counter = handle.counter(count);
                create_installers(output_dir, config, prep, &mut counter)
            },
            |_| "Prepare installer bundles".to_string(),
        )
        .await
}

/// Defaults to the manifest directory; relative output directories use the current directory.
fn resolve_output_dir(output_dir: Option<PathBuf>, manifest_dir: &Path) -> Result<PathBuf> {
    Ok(match output_dir {
        Some(path) => env::current_dir()?.join(path),
        None => manifest_dir.to_path_buf(),
    })
}
