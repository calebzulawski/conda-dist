use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    str::FromStr,
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use rattler_conda_types::{
    ChannelConfig, MatchSpec, Matches, ParseStrictness, Platform, RepoDataRecord,
};
use tempfile::TempDir;

use crate::{
    conda::{self, DEFAULT_CHANNEL, LOCKFILE_NAME},
    downloader, installer,
    progress::Progress,
    workspace::Workspace,
};

use super::{
    LockMode,
    context::{ManifestContext, load_manifest_context},
};

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
    lock_mode: LockMode,
    progress: &Progress,
) -> Result<(EnvironmentPreparation, downloader::DownloadSummary, bool)> {
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

    let lockfile_path = manifest_ctx.lockfile_path();
    let lockfile_exists = lockfile_path.exists();
    let solve_platforms_for_lock = conda::augment_with_noarch(&target_platforms);
    let existing_lock_records = if lockfile_exists {
        conda::load_locked_packages(&lockfile_path, &environment_name, &solve_platforms_for_lock)?
    } else {
        Vec::new()
    };

    let mut lock_error: Option<String> = None;

    if matches!(lock_mode, LockMode::Unlock) {
        // always regenerate the lockfile
    } else if lockfile_exists {
        match validate_lockfile(&existing_lock_records, &specs, &target_platforms) {
            Ok(()) => {}
            Err(err) => {
                lock_error = Some(err.to_string());
            }
        }
    } else if matches!(lock_mode, LockMode::Locked) {
        bail!(
            "lockfile required by --locked but not found at {}; generate it with --unlock",
            lockfile_path.display()
        );
    }

    let lock_reused =
        lockfile_exists && lock_error.is_none() && !matches!(lock_mode, LockMode::Unlock);

    if matches!(lock_mode, LockMode::Locked) && lock_error.is_some() {
        let reason = lock_error.expect("lockfile validation failed without error message");
        bail!("lockfile is out of date: {}", reason);
    }

    let virtual_package_config = manifest_ctx.config.virtual_packages();
    let total_platforms = target_platforms.len();
    let solved_records = if lock_reused {
        existing_lock_records.clone()
    } else {
        let locked_by_subdir_for_solve = build_locked_by_subdir(&existing_lock_records);
        let solve_step = progress.step("Solve environment");
        let target_platforms_for_solve = target_platforms.clone();
        let channels_for_solve = channels;
        let specs_for_solve = specs.clone();
        solve_step
            .run_with(
                Some(Duration::from_millis(120)),
                move |handle| async move {
                    let mut counter = handle.counter(total_platforms);

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

                        counter.set(index + 1);
                    }

                    Ok(combined)
                },
                |_| format!("Solve environment ({total_platforms}/{total_platforms})"),
            )
            .await?
    };

    let bundle_metadata = installer::PreparedBundleMetadata::from_config(
        &environment_name,
        manifest_ctx.config.metadata(),
        &manifest_ctx.manifest_dir,
        &solved_records,
        manifest_ctx.config.author(),
    )?;

    let package_cache_dir = workspace.package_cache_dir();

    let download_step = progress.step("Download packages");
    let solved_records_for_download = solved_records.clone();
    let channel_dir_for_download = channel_dir.clone();
    let download_summary = download_step
        .run_with(
            None,
            {
                let package_cache_dir = package_cache_dir;
                move |handle| {
                    let progress_bar = handle.progress_bar();
                    let solved_records = solved_records_for_download.clone();
                    let channel_dir = channel_dir_for_download.clone();
                    async move {
                        downloader::download_and_stage_packages(
                            &solved_records,
                            &channel_dir,
                            &package_cache_dir,
                            &progress_bar,
                        )
                        .await
                    }
                }
            },
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
        .to_path(&lockfile_path)
        .with_context(|| format!("failed to write lockfile to {}", lockfile_path.display()))?;

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

    Ok((preparation, download_summary, lock_reused))
}

pub async fn execute_lock(
    args: crate::cli::LockArgs,
    work_dir: Option<PathBuf>,
    lock_mode: LockMode,
) -> Result<()> {
    let manifest_ctx = load_manifest_context(args.manifest)?;
    let workspace = Workspace::from_manifest_dir(&manifest_ctx.manifest_dir, work_dir)?;
    let target_platforms = conda::resolve_target_platforms(manifest_ctx.config.platforms())?;
    if target_platforms.is_empty() {
        bail!("no target platforms specified");
    }

    let progress = Progress::stdout();
    let (prep, _, lock_reused) = prepare_environment(
        &manifest_ctx,
        &workspace,
        target_platforms,
        lock_mode,
        &progress,
    )
    .await?;

    // Drop the temp directory promptly
    drop(prep);

    if lock_reused {
        println!(
            "Lockfile at {} already up to date.",
            manifest_ctx.lockfile_path().display()
        );
    } else {
        println!(
            "Lockfile written to {}.",
            manifest_ctx.lockfile_path().display()
        );
    }

    Ok(())
}

fn build_locked_by_subdir(records: &[RepoDataRecord]) -> HashMap<String, Vec<RepoDataRecord>> {
    let mut by_subdir: HashMap<String, Vec<RepoDataRecord>> = HashMap::new();
    for record in records {
        by_subdir
            .entry(record.package_record.subdir.clone())
            .or_default()
            .push(record.clone());
    }
    by_subdir
}

fn validate_lockfile(
    records: &[RepoDataRecord],
    specs: &[MatchSpec],
    target_platforms: &[Platform],
) -> Result<()> {
    let mut by_platform: HashMap<Platform, Vec<RepoDataRecord>> = HashMap::new();
    for record in records {
        let platform = Platform::from_str(&record.package_record.subdir).with_context(|| {
            format!(
                "lockfile entry '{}' has an unrecognised subdir '{}'",
                record.package_record.name.as_normalized(),
                record.package_record.subdir
            )
        })?;
        by_platform
            .entry(platform)
            .or_default()
            .push(record.clone());
    }

    let all_keys: HashSet<String> = records.iter().map(lock_key).collect();
    let noarch_records = by_platform
        .get(&Platform::NoArch)
        .cloned()
        .unwrap_or_default();
    let mut covered_keys: HashSet<String> = HashSet::new();

    for platform in target_platforms {
        let mut records_for_platform = by_platform.get(platform).cloned().unwrap_or_default();
        records_for_platform.extend(noarch_records.iter().cloned());
        let visited = validate_platform_lock(*platform, records_for_platform, specs)?;
        covered_keys.extend(visited);
    }

    if covered_keys != all_keys {
        let extras = all_keys
            .difference(&covered_keys)
            .cloned()
            .collect::<Vec<_>>();
        bail!(
            "lockfile contains package(s) unrelated to manifest dependencies: {}",
            extras.join(", ")
        );
    }

    Ok(())
}

fn validate_platform_lock(
    platform: Platform,
    records: Vec<RepoDataRecord>,
    specs: &[MatchSpec],
) -> Result<HashSet<String>> {
    if records.is_empty() {
        bail!(
            "lockfile does not contain any packages for platform {}",
            platform.as_str()
        );
    }

    let mut by_name: HashMap<String, RepoDataRecord> = HashMap::new();
    for record in records {
        let name = record.package_record.name.as_normalized().to_string();
        if by_name.insert(name.clone(), record).is_some() {
            bail!(
                "lockfile contains multiple entries for package '{}' on platform {}",
                name,
                platform.as_str()
            );
        }
    }

    let mut queue: VecDeque<String> = VecDeque::new();
    let mut visited_keys: HashSet<String> = HashSet::new();

    for spec in specs {
        let spec_name = spec
            .name
            .as_ref()
            .map(|name| name.as_normalized().to_string())
            .ok_or_else(|| {
                anyhow!(
                    "manifest dependency '{}' does not specify a package name",
                    spec
                )
            })?;

        let record = by_name.get(&spec_name).ok_or_else(|| {
            anyhow!(
                "lockfile missing package '{}' required by manifest for platform {}",
                spec_name,
                platform.as_str()
            )
        })?;

        if !spec.matches(&record.package_record) {
            bail!(
                "lockfile entry for '{}' does not satisfy manifest requirement '{}' on platform {}",
                spec_name,
                spec,
                platform.as_str()
            );
        }

        queue.push_back(spec_name);
    }

    while let Some(name) = queue.pop_front() {
        let record = by_name.get(&name).ok_or_else(|| {
            anyhow!(
                "lockfile missing package '{}' while validating platform {}",
                name,
                platform.as_str()
            )
        })?;

        let key = lock_key(record);
        if !visited_keys.insert(key) {
            continue;
        }

        for dependency in &record.package_record.depends {
            if dependency.starts_with("__") {
                continue;
            }

            let dep_spec =
                MatchSpec::from_str(dependency, ParseStrictness::Lenient).with_context(|| {
                    format!(
                        "failed to parse dependency '{}' for package '{}' in lockfile",
                        dependency, name
                    )
                })?;

            let dep_name = dep_spec
                .name
                .as_ref()
                .map(|candidate| candidate.as_normalized().to_string())
                .ok_or_else(|| {
                    anyhow!(
                        "dependency '{}' for package '{}' is missing a package name",
                        dependency,
                        name
                    )
                })?;

            let dep_record = by_name.get(&dep_name).ok_or_else(|| {
                anyhow!(
                    "lockfile missing dependency '{}' required by '{}' on platform {}",
                    dep_name,
                    name,
                    platform.as_str()
                )
            })?;

            if !dep_spec.matches(&dep_record.package_record) {
                bail!(
                    "lockfile entry '{}' does not satisfy dependency '{}' of '{}' on platform {}",
                    dep_name,
                    dependency,
                    name,
                    platform.as_str()
                );
            }

            queue.push_back(dep_name);
        }
    }

    Ok(visited_keys)
}

fn lock_key(record: &RepoDataRecord) -> String {
    format!(
        "{}::{}",
        record.package_record.subdir,
        record.package_record.name.as_normalized()
    )
}
