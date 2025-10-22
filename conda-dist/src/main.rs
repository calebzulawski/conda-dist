use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
use futures::{StreamExt, TryStreamExt, stream};
use rattler::default_cache_dir;
use rattler_conda_types::{
    Channel, ChannelConfig, GenericVirtualPackage, MatchSpec, ParseStrictness, Platform,
    RepoDataRecord,
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
use serde::Deserialize;
use url::Url;

const DEFAULT_CHANNEL: &str = "conda-forge";
const LOCKFILE_NAME: &str = "conda-lock.yml";

#[derive(Debug, Parser)]
#[command(version, about = "Solve conda dependencies and produce a lockfile", long_about = None)]
struct Cli {
    /// Path to the conda-dist manifest (conda-dist.toml)
    #[arg(value_name = "MANIFEST", default_value = "conda-dist.toml")]
    manifest: PathBuf,

    /// Optional directory to place the generated environment directory
    #[arg(long = "output-dir", value_name = "DIR")]
    output_dir: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct CondaDistConfig {
    name: String,
    #[serde(default)]
    channels: Vec<String>,
    #[serde(default)]
    platforms: Vec<String>,
    dependencies: DependencySpec,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum DependencySpec {
    Table(BTreeMap<String, String>),
    List(Vec<String>),
}

impl DependencySpec {
    fn to_match_specs(&self) -> Result<Vec<MatchSpec>> {
        match self {
            DependencySpec::Table(entries) => entries
                .iter()
                .map(|(name, constraint)| {
                    let spec = if constraint.trim().is_empty() || constraint.trim() == "*" {
                        name.clone()
                    } else {
                        format!("{name} {}", constraint.trim())
                    };
                    MatchSpec::from_str(&spec, ParseStrictness::Strict)
                        .with_context(|| format!("failed to parse dependency '{spec}'"))
                })
                .collect(),
            DependencySpec::List(items) => items
                .iter()
                .map(|value| {
                    MatchSpec::from_str(value, ParseStrictness::Strict)
                        .with_context(|| format!("failed to parse dependency '{value}'"))
                })
                .collect(),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    execute(cli).await
}

async fn execute(cli: Cli) -> Result<()> {
    let manifest_path = cli.manifest;
    let manifest_path = manifest_path.canonicalize().with_context(|| {
        format!(
            "failed to resolve manifest path {}",
            manifest_path.display()
        )
    })?;

    let manifest_dir = manifest_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let manifest_raw = fs::read_to_string(&manifest_path)
        .with_context(|| format!("failed to read manifest at {}", manifest_path.display()))?;
    let config: CondaDistConfig = toml::from_str(&manifest_raw)
        .with_context(|| format!("failed to parse manifest {}", manifest_path.display()))?;

    let environment_name = config.name.as_str();
    let output_dir = cli
        .output_dir
        .unwrap_or_else(|| manifest_dir.join(environment_name));
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("failed to create output directory {}", output_dir.display()))?;

    let channel_dir = output_dir.canonicalize().with_context(|| {
        format!(
            "failed to canonicalize output directory {}",
            output_dir.display()
        )
    })?;

    let lockfile_path = channel_dir.join(LOCKFILE_NAME);

    let channel_strings = if config.channels.is_empty() {
        vec![DEFAULT_CHANNEL.to_string()]
    } else {
        config.channels.clone()
    };

    let channel_config = ChannelConfig::default_with_root_dir(manifest_dir.clone());
    let channels: Vec<Channel> = channel_strings
        .iter()
        .map(|ch| {
            Channel::from_str(ch, &channel_config)
                .with_context(|| format!("failed to parse channel '{ch}'"))
        })
        .collect::<Result<_>>()?;

    let platforms: Vec<Platform> = if config.platforms.is_empty() {
        vec![Platform::current()]
    } else {
        config
            .platforms
            .iter()
            .map(|raw| {
                Platform::from_str(raw).with_context(|| format!("failed to parse platform '{raw}'"))
            })
            .collect::<Result<_>>()?
    };

    if platforms.is_empty() {
        bail!("no target platforms specified");
    }

    let specs = config.dependencies.to_match_specs()?;
    if specs.is_empty() {
        bail!("no dependencies specified in manifest");
    }

    let overrides = VirtualPackageOverrides::default();
    let virtual_packages: Vec<GenericVirtualPackage> = VirtualPackage::detect(&overrides)?
        .into_iter()
        .map(GenericVirtualPackage::from)
        .collect();

    let gateway = build_gateway()?;

    let file_channel_url = Url::from_directory_path(&channel_dir)
        .map_err(|_| {
            anyhow!(
                "failed to convert channel directory {} to file URL",
                channel_dir.display()
            )
        })?
        .to_string();
    let channel_urls: Vec<String> = std::iter::once(file_channel_url)
        .chain(channels.iter().map(|ch| ch.base_url.to_string()))
        .collect();

    let mut solve_platforms = platforms.clone();
    if !solve_platforms.iter().any(|p| *p == Platform::NoArch) {
        solve_platforms.push(Platform::NoArch);
    }

    let repo_data_sets = gateway
        .query(
            channels.clone(),
            solve_platforms.iter().copied(),
            specs.clone(),
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
        specs: specs.clone(),
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

    download_and_index_packages(&solved_records, &channel_dir).await?;

    let lock_file = build_lockfile(environment_name, &channel_urls, &solved_records)?;
    lock_file
        .to_path(&lockfile_path)
        .with_context(|| format!("failed to write lockfile to {}", lockfile_path.display()))?;

    println!(
        "created environment '{}' at {}",
        environment_name,
        channel_dir.display()
    );
    println!("lockfile written to {}", lockfile_path.display());
    println!(
        "downloaded {} packages into {}",
        solved_records.len(),
        channel_dir.display()
    );

    Ok(())
}

fn build_gateway() -> Result<Gateway> {
    let mut builder = GatewayBuilder::new();
    #[cfg(not(target_arch = "wasm32"))]
    {
        let cache_root = default_cache_dir()?.join("repodata");
        builder.set_cache_dir(&cache_root);
    }

    Ok(builder.finish())
}

async fn download_and_index_packages(records: &[RepoDataRecord], channel_dir: &Path) -> Result<()> {
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

    stream::iter(entries.into_iter())
        .map(|entry| {
            let client = client.clone();
            let channel_dir = channel_dir.to_path_buf();
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

                Ok::<(), anyhow::Error>(())
            }
        })
        .buffer_unordered(8)
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

    Ok(())
}

fn build_lockfile(
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
