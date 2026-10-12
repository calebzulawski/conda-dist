use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use rattler_conda_types::{Channel, MatchSpec, Platform, RepoDataRecord};
use rattler_lock::{CondaPackageData, LockFile, LockFileBuilder, SolveOptions};
use rattler_solve::{ChannelPriority, SolveStrategy};
use tempfile::TempDir;

use super::{
    LockMode,
    context::{ManifestContext, load_manifest_context},
    installer,
};
use crate::{
    conda::{self, LOCKFILE_NAME},
    config::CondaDistConfig,
    downloader,
    progress::Progress,
    workspace::Workspace,
};

#[derive(Debug)]
pub struct EnvironmentPreparation {
    pub environment_name: String,
    pub staging_dir: TempDir,
    pub channel_dir: PathBuf,
    pub bundle_metadata: installer::BundleMetadataManifest,
    pub target_platforms: Vec<Platform>,
    /// Installer lock with fixed names for the full solution, base, and components.
    pub lockfile: LockFile,
}

/// Reuses or solves complete package sets for all manifest platforms and writes the component lockfile.
/// The boolean reports whether the existing full solution was reused.
async fn resolve_environment(
    manifest: &ManifestContext,
    lock_mode: LockMode,
    progress: &Progress,
) -> Result<(LockFile, bool)> {
    let config = &manifest.config;
    let channels = config.channels()?;
    let path = manifest.lockfile_path();

    // Combine base and component requirements.
    let base_specs = config.dependencies().to_match_specs()?;
    let mut requested = base_specs.clone();
    for component in config.components().values() {
        requested.extend(component.dependencies.to_match_specs()?);
    }
    if requested.is_empty() {
        bail!("no dependencies specified in manifest");
    }

    // Load the existing lockfile for validation or solver preferences.
    let locked = if path.exists() {
        Some(
            LockFile::from_path(&path)
                .with_context(|| format!("failed to load lockfile from {}", path.display()))?,
        )
    } else {
        None
    };

    let validated = match (lock_mode, locked.as_ref()) {
        (LockMode::Locked, None) => {
            bail!(
                "lockfile required by --locked but not found at {}; generate it with --unlock",
                path.display()
            );
        }
        (LockMode::Locked, Some(lockfile)) => Some(
            validate_lockfile(config, lockfile, &requested)
                .map_err(|error| anyhow!("lockfile is out of date: {error}"))?,
        ),
        (LockMode::Auto, Some(lockfile)) => validate_lockfile(config, lockfile, &requested).ok(),
        (LockMode::Auto, None) | (LockMode::Unlock, _) => None,
    };
    let reused = validated.is_some();
    let solved = match validated {
        Some(records) => records,
        None => solve_platforms(config, &channels, &requested, locked.as_ref(), progress).await?,
    };

    // Rebuild base and component subsets even when the full solution was reused.
    let lock = build_component_lockfile(config, &channels, &base_specs, solved)?;
    lock.to_path(&path)
        .with_context(|| format!("failed to write lockfile to {}", path.display()))?;
    Ok((lock, reused))
}

/// Solves combined base and component requirements per platform, including `noarch` packages.
/// Packages from the previous lockfile are preferences for the new solve.
async fn solve_platforms(
    config: &CondaDistConfig,
    channels: &[Channel],
    requested: &[MatchSpec],
    previous: Option<&LockFile>,
    progress: &Progress,
) -> Result<HashMap<Platform, Vec<RepoDataRecord>>> {
    let gateway = conda::build_gateway()?;
    let count = config.platforms().len();
    progress
        .step("Solve environment")
        .run_with(
            Some(Duration::from_millis(120)),
            |handle| async move {
                let mut counter = handle.counter(count);
                let mut solved = HashMap::new();
                for (index, platform) in config.platforms().iter().enumerate() {
                    let solve_platforms = if *platform == Platform::NoArch {
                        vec![*platform]
                    } else {
                        vec![*platform, Platform::NoArch]
                    };
                    let overrides = config
                        .virtual_packages()
                        .and_then(|cfg| cfg.for_platform(*platform));
                    let virtual_packages =
                        conda::detect_virtual_packages_for_platform(*platform, overrides)?;
                    let locked_packages = match previous {
                        Some(lockfile) => {
                            conda::locked_packages(lockfile, config.name(), *platform)?
                        }
                        None => Vec::new(),
                    };
                    let records = conda::solve_environment(
                        &gateway,
                        channels,
                        requested,
                        &solve_platforms,
                        locked_packages,
                        virtual_packages,
                    )
                    .await
                    .with_context(|| {
                        format!("failed to solve environment for platform {platform}")
                    })?;
                    solved.insert(*platform, records);
                    counter.set(index + 1);
                }
                Ok(solved)
            },
            |_| format!("Solve environment ({count}/{count})"),
        )
        .await
}

/// Builds full, base, and component lock environments from each platform's solved package pool.
/// Subset solves use only that pool, keeping shared package versions and builds consistent.
fn build_component_lockfile(
    config: &CondaDistConfig,
    channels: &[Channel],
    base_specs: &[MatchSpec],
    solved: HashMap<Platform, Vec<RepoDataRecord>>,
) -> Result<LockFile> {
    let environment_name = config.name();
    let channel_urls: Vec<_> = std::iter::once("file:./".to_string())
        .chain(channels.iter().map(|ch| ch.base_url.to_string()))
        .collect();
    let mut builder = LockFileBuilder::new();
    for (platform, records) in solved {
        let overrides = config
            .virtual_packages()
            .and_then(|cfg| cfg.for_platform(platform));
        let virtual_packages = conda::detect_virtual_packages_for_platform(platform, overrides)?;
        let solve = |specs: &[MatchSpec]| {
            conda::solver::solve_from_records(&records, specs, &virtual_packages)
        };
        let mut add_environment = |name: &str, packages: &[RepoDataRecord]| {
            builder.set_channels(name, channel_urls.iter().map(String::as_str));
            builder.set_options(
                name,
                SolveOptions {
                    strategy: SolveStrategy::Highest,
                    channel_priority: ChannelPriority::Strict,
                    exclude_newer: None,
                },
            );
            for record in packages {
                builder.add_conda_package(name, platform, CondaPackageData::from(record.clone()));
            }
        };
        add_environment(environment_name, &records);
        add_environment(&format!("{environment_name}:base"), &solve(base_specs)?);
        for (name, component) in config.components() {
            let mut specs = base_specs.to_vec();
            for member in component.resolved_components() {
                specs.extend(config.components()[member].dependencies.to_match_specs()?);
            }
            add_environment(
                &format!("{environment_name}:component:{name}"),
                &solve(&specs)?,
            );
        }
    }
    Ok(builder.finish())
}

/// Checks that current requirements can be solved using only the locked packages.
/// Rejects packages no longer required by the current manifest.
/// Returns the validated full package sets for reuse.
fn validate_lockfile(
    config: &CondaDistConfig,
    lockfile: &LockFile,
    requested: &[MatchSpec],
) -> Result<HashMap<Platform, Vec<RepoDataRecord>>> {
    let mut validated = HashMap::new();
    for platform in config.platforms() {
        let records = conda::locked_packages(lockfile, config.name(), *platform)?;
        let overrides = config
            .virtual_packages()
            .and_then(|cfg| cfg.for_platform(*platform));
        let virtual_packages = conda::detect_virtual_packages_for_platform(*platform, overrides)?;
        let solution = conda::solver::solve_from_records(&records, requested, &virtual_packages)?;
        if solution.len() != records.len() {
            bail!(
                "lockfile contains packages no longer required by the manifest on platform {platform}"
            );
        }
        validated.insert(*platform, records);
    }
    Ok(validated)
}

/// Resolves all manifest platforms and stages all components' packages for the selected targets.
pub async fn prepare_environment(
    manifest_ctx: &ManifestContext,
    workspace: &Workspace,
    target_platforms: Vec<Platform>,
    lock_mode: LockMode,
    progress: &Progress,
) -> Result<EnvironmentPreparation> {
    for platform in &target_platforms {
        if !manifest_ctx.config.platforms().contains(platform) {
            bail!("selected platform '{platform}' is not listed in the manifest platforms");
        }
    }
    let (lockfile, _) = resolve_environment(manifest_ctx, lock_mode, progress).await?;
    let environment_name = manifest_ctx.config.name().to_string();
    let lockfile = conda::lockfile::build_bundle_lockfile(&lockfile, &environment_name);
    let staging_dir = tempfile::tempdir().context("failed to create staging directory")?;
    let channel_dir = staging_dir.path().join(&environment_name);
    tokio::fs::create_dir_all(&channel_dir).await?;
    let records = packages_to_stage(&lockfile, &target_platforms)?;
    let mut bundle_metadata = installer::BundleMetadataManifest::from_config(
        &environment_name,
        manifest_ctx.config.metadata(),
        &records,
        manifest_ctx.config.author(),
    )?;
    bundle_metadata.components = manifest_ctx
        .config
        .components()
        .iter()
        .map(|(name, component)| (name.clone(), component.includes.iter().cloned().collect()))
        .collect();
    let package_cache_dir = workspace.package_cache_dir();
    let channel_for_download = &channel_dir;
    progress
        .step("Download packages")
        .run_with(
            None,
            |handle| async move {
                downloader::download_and_stage_packages(
                    &records,
                    channel_for_download,
                    &package_cache_dir,
                    &handle.progress_bar(),
                )
                .await
            },
            |summary| {
                format!(
                    "Download packages ({}/{}; {} cached)",
                    summary.total_packages,
                    summary.total_packages,
                    summary.total_packages - summary.fetched_packages,
                )
            },
        )
        .await?;
    lockfile.to_path(&channel_dir.join(LOCKFILE_NAME))?;
    Ok(EnvironmentPreparation {
        environment_name,
        staging_dir,
        channel_dir,
        bundle_metadata,
        target_platforms,
        lockfile,
    })
}

/// Collects selected targets' bundled packages, deduplicated by subdirectory and archive filename.
/// Rejects different URLs or checksums for the same archive path.
fn packages_to_stage(lockfile: &LockFile, platforms: &[Platform]) -> Result<Vec<RepoDataRecord>> {
    let mut staged = BTreeMap::<(String, String), RepoDataRecord>::new();
    for platform in platforms {
        for record in conda::locked_packages(lockfile, "default", *platform)? {
            let key = (
                record.package_record.subdir.clone(),
                record.file_name.clone(),
            );
            if let Some(other) = staged.get(&key)
                && (other.package_record.sha256 != record.package_record.sha256
                    || other.package_record.md5 != record.package_record.md5
                    || other.url != record.url)
            {
                bail!(
                    "different locked packages share archive path {}/{}",
                    key.0,
                    key.1
                );
            }
            staged.insert(key, record);
        }
    }
    Ok(staged.into_values().collect())
}

/// Updates or validates the manifest lockfile and reports whether the full solution was reused.
pub async fn execute_lock(
    args: crate::cli::LockArgs,
    _work_dir: Option<PathBuf>,
    lock_mode: LockMode,
) -> Result<()> {
    let manifest = load_manifest_context(args.manifest)?;
    let (_, reused) = resolve_environment(&manifest, lock_mode, &Progress::stdout()).await?;
    if reused {
        println!(
            "Lockfile at {} already up to date.",
            manifest.lockfile_path().display()
        );
    } else {
        println!(
            "Lockfile written to {}.",
            manifest.lockfile_path().display()
        );
    }
    Ok(())
}
