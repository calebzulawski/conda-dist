use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use anyhow::{Context, Result, bail};
use futures::{StreamExt, TryStreamExt, stream};
use indicatif::ProgressBar;
use rattler_conda_types::{Platform, RepoDataRecord};
use rattler_digest::{Sha256, Sha256Hash, compute_bytes_digest};
use rattler_index::{IndexFsConfig, index_fs};
use reqwest::Client;
use tokio::fs;

const MAX_PARALLEL_DOWNLOADS: usize = 8;

#[derive(Debug, Default, Clone)]
pub struct DownloadSummary {
    pub total_packages: usize,
    pub fetched_packages: usize,
}

#[derive(Clone)]
struct PackageEntry {
    subdir: String,
    file_name: String,
    url: String,
    sha256: Option<Sha256Hash>,
}

pub async fn download_and_stage_packages(
    records: &[RepoDataRecord],
    channel_dir: &Path,
    cache_dir: &Path,
    progress: &ProgressBar,
) -> Result<DownloadSummary> {
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
                sha256: record.package_record.sha256,
            });
        }
    }

    fs::create_dir_all(channel_dir).await.with_context(|| {
        format!(
            "failed to prepare channel directory {}",
            channel_dir.display()
        )
    })?;
    fs::create_dir_all(cache_dir)
        .await
        .with_context(|| format!("failed to prepare cache directory {}", cache_dir.display()))?;

    let total_packages = entries.len();
    progress.set_message(format!("Download packages (0/{total_packages})"));
    progress.tick();

    if total_packages == 0 {
        ensure_noarch(channel_dir).await?;
        index_channel(channel_dir).await?;
        return Ok(DownloadSummary {
            total_packages: 0,
            fetched_packages: 0,
        });
    }

    let client = Client::builder()
        .user_agent("conda-dist/0.1.0")
        .build()
        .context("failed to construct HTTP client")?;

    let completed = Arc::new(AtomicUsize::new(0));
    let fetched = Arc::new(AtomicUsize::new(0));
    let channel_dir = channel_dir.to_path_buf();
    let cache_dir = cache_dir.to_path_buf();

    stream::iter(entries.into_iter())
        .map(|entry| {
            let client = client.clone();
            let channel_dir = channel_dir.clone();
            let cache_dir = cache_dir.clone();
            let progress = progress.clone();
            let completed = completed.clone();
            let fetched = fetched.clone();
            let ctx = StageContext::new(
                client,
                channel_dir,
                cache_dir,
                progress,
                completed,
                fetched,
                total_packages,
            );
            async move { stage_package(entry, ctx).await }
        })
        .buffer_unordered(MAX_PARALLEL_DOWNLOADS)
        .try_collect::<()>()
        .await?;

    ensure_noarch(&channel_dir).await?;
    index_channel(&channel_dir).await?;

    Ok(DownloadSummary {
        total_packages,
        fetched_packages: fetched.load(Ordering::Relaxed),
    })
}

struct StageContext {
    client: Client,
    channel_dir: PathBuf,
    cache_dir: PathBuf,
    progress: ProgressBar,
    completed: Arc<AtomicUsize>,
    fetched: Arc<AtomicUsize>,
    total_packages: usize,
}

impl StageContext {
    fn new(
        client: Client,
        channel_dir: PathBuf,
        cache_dir: PathBuf,
        progress: ProgressBar,
        completed: Arc<AtomicUsize>,
        fetched: Arc<AtomicUsize>,
        total_packages: usize,
    ) -> Self {
        Self {
            client,
            channel_dir,
            cache_dir,
            progress,
            completed,
            fetched,
            total_packages,
        }
    }
}

async fn stage_package(entry: PackageEntry, ctx: StageContext) -> Result<()> {
    let PackageEntry {
        subdir,
        file_name,
        url,
        sha256,
    } = entry;

    let channel_subdir = ctx.channel_dir.join(&subdir);
    fs::create_dir_all(&channel_subdir).await.with_context(|| {
        format!(
            "failed to create channel subdir {}",
            channel_subdir.display()
        )
    })?;

    let cache_subdir = ctx.cache_dir.join(&subdir);
    fs::create_dir_all(&cache_subdir)
        .await
        .with_context(|| format!("failed to create cache subdir {}", cache_subdir.display()))?;

    let cached_path = cache_subdir.join(&file_name);
    let staged_path = channel_subdir.join(&file_name);

    let cache_ready = verify_cached_package(&cached_path, sha256).await?;
    let mut downloaded = false;
    if !cache_ready {
        download_to_cache(&ctx.client, &url, &cached_path, sha256).await?;
        downloaded = true;
    }

    copy_into_channel(&cached_path, &staged_path).await?;

    if downloaded {
        ctx.fetched.fetch_add(1, Ordering::Relaxed);
    }

    let done = ctx.completed.fetch_add(1, Ordering::Relaxed) + 1;
    ctx.progress
        .set_message(format!("Download packages ({done}/{})", ctx.total_packages));
    ctx.progress.tick();

    Ok(())
}

async fn verify_cached_package(path: &Path, expected: Option<Sha256Hash>) -> Result<bool> {
    if expected.is_none() {
        return Ok(false);
    }

    if fs::metadata(path).await.is_err() {
        return Ok(false);
    }

    let expected = expected.expect("checked above");
    let bytes = fs::read(path)
        .await
        .with_context(|| format!("failed to read cached package {}", path.display()))?;
    let computed = compute_bytes_digest::<Sha256>(&bytes);
    if computed == expected {
        Ok(true)
    } else {
        fs::remove_file(path).await.ok();
        Ok(false)
    }
}

async fn download_to_cache(
    client: &Client,
    url: &str,
    cached_path: &Path,
    sha256: Option<Sha256Hash>,
) -> Result<()> {
    let temp_path = cached_path.with_extension("part");
    if fs::metadata(&temp_path).await.is_ok() {
        fs::remove_file(&temp_path)
            .await
            .with_context(|| format!("failed to remove {}", temp_path.display()))?;
    }

    let response = client
        .get(url)
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

    fs::write(&temp_path, &bytes)
        .await
        .with_context(|| format!("failed to write {}", temp_path.display()))?;
    if fs::metadata(&cached_path).await.is_ok() {
        fs::remove_file(&cached_path)
            .await
            .with_context(|| format!("failed to replace {}", cached_path.display()))?;
    }
    fs::rename(&temp_path, cached_path)
        .await
        .with_context(|| format!("failed to persist {}", cached_path.display()))?;

    Ok(())
}

async fn copy_into_channel(cache_path: &Path, staged_path: &Path) -> Result<()> {
    let temp_path = staged_path.with_extension("part");
    if fs::metadata(&temp_path).await.is_ok() {
        fs::remove_file(&temp_path)
            .await
            .with_context(|| format!("failed to remove {}", temp_path.display()))?;
    }

    fs::copy(cache_path, &temp_path)
        .await
        .with_context(|| format!("failed to copy cached package {}", cache_path.display()))?;
    if fs::metadata(staged_path).await.is_ok() {
        fs::remove_file(staged_path)
            .await
            .with_context(|| format!("failed to replace {}", staged_path.display()))?;
    }
    fs::rename(&temp_path, staged_path)
        .await
        .with_context(|| format!("failed to persist {}", staged_path.display()))?;

    Ok(())
}

async fn ensure_noarch(channel_dir: &Path) -> Result<()> {
    let noarch_dir = channel_dir.join(Platform::NoArch.as_str());
    fs::create_dir_all(&noarch_dir).await.with_context(|| {
        format!(
            "failed to ensure noarch subdir in {}",
            channel_dir.display()
        )
    })
}

async fn index_channel(channel_dir: &Path) -> Result<()> {
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
    .context("failed to index downloaded packages")
}
