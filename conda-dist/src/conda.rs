use std::{
    collections::HashSet,
    path::Path,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use anyhow::{Context, Result, bail};
use futures::{StreamExt, TryStreamExt, stream};
use indicatif::ProgressBar;
use rattler::default_cache_dir;
use rattler_conda_types::{
    Channel, ChannelConfig, GenericVirtualPackage, MatchSpec, Platform, RepoDataRecord,
};
use rattler_digest::{Sha256, Sha256Hash, compute_bytes_digest};
use rattler_index::{IndexFsConfig, index_fs};
use rattler_lock::{CondaPackageData, LockFile, LockFileBuilder, SolveOptions};
use rattler_repodata_gateway::{Gateway, GatewayBuilder};
use rattler_solve::{
    ChannelPriority, RepoDataIter, SolveStrategy, SolverImpl, SolverTask, resolvo,
};
use rattler_virtual_packages::{VirtualPackage, VirtualPackageOverrides};
use reqwest::Client;

pub const DEFAULT_CHANNEL: &str = "conda-forge";
pub const LOCKFILE_NAME: &str = "conda-lock.yml";

pub fn parse_channels(channel_strings: &[String], config: &ChannelConfig) -> Result<Vec<Channel>> {
    channel_strings
        .iter()
        .map(|ch| {
            Channel::from_str(ch, config).with_context(|| format!("failed to parse channel '{ch}'"))
        })
        .collect()
}

pub fn resolve_target_platforms(raw_platforms: &[String]) -> Result<Vec<Platform>> {
    if raw_platforms.is_empty() {
        return Ok(vec![Platform::current()]);
    }

    raw_platforms
        .iter()
        .map(|value| {
            Platform::from_str(value).with_context(|| format!("failed to parse platform '{value}'"))
        })
        .collect()
}

pub fn augment_with_noarch(platforms: &[Platform]) -> Vec<Platform> {
    let mut solve_platforms = platforms.to_vec();
    if !solve_platforms
        .iter()
        .any(|platform| *platform == Platform::NoArch)
    {
        solve_platforms.push(Platform::NoArch);
    }
    solve_platforms
}

pub fn detect_virtual_packages() -> Result<Vec<GenericVirtualPackage>> {
    let overrides = VirtualPackageOverrides::default();
    let packages = VirtualPackage::detect(&overrides)?
        .into_iter()
        .map(GenericVirtualPackage::from)
        .collect();
    Ok(packages)
}

pub fn build_gateway() -> Result<Gateway> {
    let mut builder = GatewayBuilder::new();
    #[cfg(not(target_arch = "wasm32"))]
    {
        let cache_root = default_cache_dir()?.join("repodata");
        builder.set_cache_dir(&cache_root);
    }

    Ok(builder.finish())
}

pub async fn solve_environment(
    gateway: &Gateway,
    channels: &[Channel],
    specs: &[MatchSpec],
    solve_platforms: &[Platform],
    virtual_packages: Vec<GenericVirtualPackage>,
) -> Result<Vec<RepoDataRecord>> {
    let repo_data_sets = gateway
        .query(
            channels.to_vec(),
            solve_platforms.iter().copied(),
            specs.to_vec(),
        )
        .recursive(true)
        .execute()
        .await?;

    if repo_data_sets.is_empty() {
        bail!("the solver received no available packages; check your channels and platforms");
    }

    let available_packages: Vec<_> = repo_data_sets
        .iter()
        .map(|data| RepoDataIter(data.iter()))
        .collect();

    let mut solver = resolvo::Solver::default();
    let solve_result = solver.solve(SolverTask {
        available_packages,
        locked_packages: Vec::new(),
        pinned_packages: Vec::new(),
        virtual_packages,
        specs: specs.to_vec(),
        constraints: Vec::new(),
        timeout: None,
        channel_priority: ChannelPriority::Strict,
        exclude_newer: None,
        strategy: SolveStrategy::Highest,
    })?;

    let mut solved_records = solve_result.records;

    #[cfg(not(target_arch = "wasm32"))]
    {
        gateway
            .ensure_run_exports(solved_records.iter_mut(), None)
            .await
            .context("failed to enrich run exports for solved records")?;
    }

    Ok(solved_records)
}

pub async fn download_and_index_packages(
    records: &[RepoDataRecord],
    channel_dir: &Path,
    progress: &ProgressBar,
) -> Result<usize> {
    const MAX_PARALLEL_DOWNLOADS: usize = 8;

    #[derive(Clone)]
    struct PackageEntry {
        subdir: String,
        file_name: String,
        url: String,
        sha256: Option<Sha256Hash>,
    }

    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for record in records {
        let key = (
            record.package_record.subdir.clone(),
            record.file_name.clone(),
        );
        if seen.insert(key.clone()) {
            entries.push(PackageEntry {
                subdir: key.0,
                file_name: key.1,
                url: record.url.to_string(),
                sha256: record.package_record.sha256.clone(),
            });
        }
    }

    tokio::fs::create_dir_all(channel_dir)
        .await
        .with_context(|| {
            format!(
                "failed to prepare channel directory {}",
                channel_dir.display()
            )
        })?;

    let client = Client::builder()
        .user_agent("conda-dist/0.1.0")
        .build()
        .context("failed to construct HTTP client")?;

    let channel_dir_buf = channel_dir.to_path_buf();
    let total_downloads = entries.len();
    progress.set_message(format!("Download packages (0/{total_downloads})"));
    progress.tick();

    if total_downloads == 0 {
        return Ok(0);
    }

    let completed = Arc::new(AtomicUsize::new(0));
    stream::iter(entries.into_iter())
        .map(|entry| {
            let client = client.clone();
            let channel_dir = channel_dir_buf.clone();
            let progress = progress.clone();
            let completed = completed.clone();
            async move {
                let PackageEntry {
                    subdir,
                    file_name,
                    url,
                    sha256,
                } = entry;

                let subdir_dir = channel_dir.join(&subdir);
                tokio::fs::create_dir_all(&subdir_dir)
                    .await
                    .with_context(|| {
                        format!("failed to create channel subdir {}", subdir_dir.display())
                    })?;

                let target_path = subdir_dir.join(&file_name);
                let temp_path = target_path.with_extension("part");
                if tokio::fs::metadata(&temp_path).await.is_ok() {
                    let _ = tokio::fs::remove_file(&temp_path).await;
                }

                let response = client
                    .get(url.clone())
                    .send()
                    .await
                    .with_context(|| format!("failed to download {url}"))?
                    .error_for_status()
                    .with_context(|| format!("request returned error status for {url}"))?;

                let bytes = response
                    .bytes()
                    .await
                    .with_context(|| format!("failed to read response body for {url}"))?;

                if let Some(expected) = sha256 {
                    let computed = compute_bytes_digest::<Sha256>(&bytes);
                    if computed != expected {
                        bail!("downloaded package '{url}' failed checksum validation");
                    }
                }

                tokio::fs::write(&temp_path, &bytes)
                    .await
                    .with_context(|| format!("failed to write {}", temp_path.display()))?;
                tokio::fs::rename(&temp_path, &target_path)
                    .await
                    .with_context(|| format!("failed to persist {}", target_path.display()))?;

                let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
                progress.set_message(format!("Download packages ({done}/{total_downloads})"));
                progress.tick();

                Ok::<(), anyhow::Error>(())
            }
        })
        .buffer_unordered(MAX_PARALLEL_DOWNLOADS)
        .try_collect::<()>()
        .await?;

    tokio::fs::create_dir_all(channel_dir.join(Platform::NoArch.as_str()))
        .await
        .with_context(|| {
            format!(
                "failed to ensure noarch subdir in channel {}",
                channel_dir.display()
            )
        })?;

    index_fs(IndexFsConfig {
        channel: channel_dir.to_path_buf(),
        target_platform: None,
        repodata_patch: None,
        write_zst: false,
        write_shards: false,
        force: true,
        max_parallel: 8,
        multi_progress: None,
    })
    .await
    .context("failed to index downloaded packages")?;

    Ok(total_downloads)
}

pub fn build_lockfile(
    environment_name: &str,
    channel_urls: &[String],
    records: &[RepoDataRecord],
) -> Result<LockFile> {
    let mut builder = LockFileBuilder::new();
    builder.set_channels(environment_name, channel_urls.iter().map(String::as_str));
    builder.set_options(
        environment_name,
        SolveOptions {
            strategy: SolveStrategy::Highest,
            channel_priority: ChannelPriority::Strict,
            exclude_newer: None,
        },
    );

    for record in records {
        let platform = Platform::from_str(&record.package_record.subdir).with_context(|| {
            format!(
                "failed to parse platform '{}'",
                record.package_record.subdir
            )
        })?;
        let package = CondaPackageData::from(record.clone());
        builder.add_conda_package(environment_name, platform, package);
    }

    Ok(builder.finish())
}
