use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::{Cursor, Write},
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use clap::Parser;
use flate2::{Compression, write::GzEncoder};
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
use tar::{Builder, EntryType, Header, HeaderMode};

include!(concat!(env!("OUT_DIR"), "/installers.rs"));

const DEFAULT_CHANNEL: &str = "conda-forge";
const LOCKFILE_NAME: &str = "conda-lock.yml";

#[derive(Debug, Parser)]
#[command(version, about = "Solve conda dependencies and produce a lockfile", long_about = None)]
struct Cli {
    /// Path to the conda-dist manifest (conda-dist.toml)
    #[arg(value_name = "MANIFEST", default_value = "conda-dist.toml")]
    manifest: PathBuf,

    /// Optional path to write the self-extracting installer script
    #[arg(long = "output", alias = "output-dir", value_name = "PATH")]
    output: Option<PathBuf>,

    /// Select which installer platform(s) to build
    #[arg(
        long = "installer-platform",
        value_name = "PLATFORM",
        default_value = "all"
    )]
    installer_platform: InstallerPlatformSelection,
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

#[derive(Debug, Clone)]
enum InstallerPlatformSelection {
    All,
    Host,
    Specific(Platform),
}

impl FromStr for InstallerPlatformSelection {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        match trimmed.to_ascii_lowercase().as_str() {
            "all" => Ok(Self::All),
            "host" => Ok(Self::Host),
            _ => Platform::from_str(trimmed)
                .map(Self::Specific)
                .map_err(|err| err.to_string()),
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
    let script_path = resolve_script_path(
        cli.output
            .unwrap_or_else(|| manifest_dir.join(format!("{environment_name}.sh"))),
        environment_name,
    )?;

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

    let target_platforms: Vec<Platform> = if config.platforms.is_empty() {
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

    if target_platforms.is_empty() {
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

    // The local channel is embedded alongside the archive, so reference it relative to the lockfile.
    let channel_urls: Vec<String> = std::iter::once("file:./".to_string())
        .chain(channels.iter().map(|ch| ch.base_url.to_string()))
        .collect();

    let mut solve_platforms = target_platforms.clone();
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

    let installer_platforms =
        resolve_installer_platforms(cli.installer_platform, &target_platforms)?;
    emit_installers(
        environment_name,
        &script_path,
        &channel_dir,
        &installer_platforms,
    )?;

    Ok(())
}

fn resolve_script_path(requested: PathBuf, environment_name: &str) -> Result<PathBuf> {
    if requested.exists() {
        let metadata = fs::metadata(&requested).with_context(|| {
            format!(
                "failed to inspect requested output path {}",
                requested.display()
            )
        })?;
        if metadata.is_dir() {
            return Ok(requested.join(format!("{environment_name}.sh")));
        }
    }

    Ok(requested)
}

fn resolve_installer_platforms(
    selection: InstallerPlatformSelection,
    manifest_platforms: &[Platform],
) -> Result<Vec<Platform>> {
    let mut ordered_unique = Vec::new();
    let mut seen = HashSet::new();
    for platform in manifest_platforms {
        if *platform == Platform::NoArch {
            continue;
        }
        let key = platform.as_str().to_string();
        if seen.insert(key) {
            ordered_unique.push(*platform);
        }
    }

    match selection {
        InstallerPlatformSelection::All => {
            if ordered_unique.is_empty() {
                bail!("no non-noarch platforms available to build installers for");
            }
            Ok(ordered_unique)
        }
        InstallerPlatformSelection::Host => {
            let host = Platform::current();
            if ordered_unique.iter().any(|p| *p == host) {
                Ok(vec![host])
            } else {
                bail!(
                    "host platform '{}' is not listed in the manifest platforms",
                    host.as_str()
                );
            }
        }
        InstallerPlatformSelection::Specific(platform) => {
            if platform == Platform::NoArch {
                bail!("cannot build installer for 'noarch' platform");
            }

            if manifest_platforms.iter().any(|p| *p == platform) {
                Ok(vec![platform])
            } else {
                bail!(
                    "selected platform '{}' is not listed in the manifest platforms",
                    platform.as_str()
                );
            }
        }
    }
}

fn embedded_installer_for_platform(platform: Platform) -> Option<&'static [u8]> {
    let key = platform.as_str();
    INSTALLERS
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, bytes)| *bytes)
}

fn emit_installers(
    environment_name: &str,
    script_path: &Path,
    channel_dir: &Path,
    selected_platforms: &[Platform],
) -> Result<()> {
    let output_dir = installer_output_directory(script_path)?;

    if script_path.exists() && !script_path.is_dir() {
        fs::remove_file(script_path)
            .or_else(|err| {
                if err.kind() == std::io::ErrorKind::NotFound {
                    Ok(())
                } else {
                    Err(err)
                }
            })
            .with_context(|| {
                format!(
                    "failed to remove obsolete installer {}",
                    script_path.display()
                )
            })?;
    }

    fs::create_dir_all(&output_dir).with_context(|| {
        format!(
            "failed to prepare installer output directory {}",
            output_dir.display()
        )
    })?;

    for platform in selected_platforms {
        let installer_bytes = embedded_installer_for_platform(*platform).with_context(|| {
            format!(
                "no embedded installer available for platform {}",
                platform.as_str()
            )
        })?;

        let archive_bytes =
            create_tar_gz_for_platform(channel_dir, environment_name, *platform, installer_bytes)
                .with_context(|| {
                format!(
                    "failed to prepare archive for platform {}",
                    platform.as_str()
                )
            })?;

        let installer_name = format!("{environment_name}-{}.sh", platform.as_str());
        let target_path = output_dir.join(installer_name);
        write_self_extracting_script(&target_path, environment_name, &archive_bytes)
            .with_context(|| format!("failed to write installer {}", target_path.display()))?;
        println!(
            "self-extracting installer written to {}",
            target_path.display()
        );
    }

    Ok(())
}

fn installer_output_directory(script_path: &Path) -> Result<PathBuf> {
    if script_path.is_dir() {
        return Ok(script_path.to_path_buf());
    }

    if !script_path.exists() && script_path.extension().is_none() {
        return Ok(script_path.to_path_buf());
    }

    if let Some(parent) = script_path.parent() {
        if parent.as_os_str().is_empty() {
            Ok(PathBuf::from("."))
        } else {
            Ok(parent.to_path_buf())
        }
    } else {
        Ok(PathBuf::from("."))
    }
}

fn create_tar_gz_for_platform(
    root_dir: &Path,
    root_name: &str,
    platform: Platform,
    installer_bytes: &[u8],
) -> Result<Vec<u8>> {
    let encoder = GzEncoder::new(Vec::new(), Compression::new(6));
    let mut builder = Builder::new(encoder);
    builder.mode(HeaderMode::Deterministic);

    builder
        .append_dir(root_name, root_dir)
        .with_context(|| format!("failed to add root directory {}", root_dir.display()))?;

    let lockfile_path = root_dir.join(LOCKFILE_NAME);
    if lockfile_path.exists() {
        builder
            .append_path_with_name(&lockfile_path, format!("{root_name}/{LOCKFILE_NAME}"))
            .with_context(|| {
                format!(
                    "failed to add lockfile {} to archive",
                    lockfile_path.display()
                )
            })?;
    }

    for entry in fs::read_dir(root_dir)
        .with_context(|| format!("failed to read directory {}", root_dir.display()))?
    {
        let entry =
            entry.with_context(|| format!("failed to read entry within {}", root_dir.display()))?;
        let file_type = entry.file_type().with_context(|| {
            format!(
                "failed to determine file type for {}",
                entry.path().display()
            )
        })?;
        if file_type.is_file() {
            let file_name = entry.file_name();
            let file_name_str = file_name.to_string_lossy();
            if file_name_str == LOCKFILE_NAME {
                continue;
            }
            let archive_path = format!("{root_name}/{file_name_str}");
            builder
                .append_path_with_name(entry.path(), archive_path)
                .with_context(|| {
                    format!("failed to add file {} to archive", entry.path().display())
                })?;
        }
    }

    let mut subdirs = vec![Platform::NoArch.as_str(), platform.as_str()];
    subdirs.sort_unstable();
    subdirs.dedup();

    for subdir in subdirs {
        let path = root_dir.join(subdir);
        if path.exists() {
            builder
                .append_dir_all(format!("{root_name}/{subdir}"), &path)
                .with_context(|| format!("failed to add {} to archive", path.display()))?;
        }
    }

    let installer_archive_path = format!("{root_name}/installer");
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Regular);
    header.set_mode(0o755);
    header.set_size(installer_bytes.len() as u64);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_cksum();
    let mut cursor = Cursor::new(installer_bytes);
    builder.append_data(&mut header, installer_archive_path, &mut cursor)?;

    let encoder = builder
        .into_inner()
        .context("failed to finalize tar archive")?;
    let archive = encoder
        .finish()
        .context("failed to complete gzip compression")?;
    Ok(archive)
}

fn installer_prologue(environment_name: &str) -> String {
    format!(
        r#"#!/bin/sh
set -eu

env_name=$(cat <<'CONDADIST_ENV_NAME'
{env_name}
CONDADIST_ENV_NAME
)

temp_dir="$(mktemp -d)"
cleanup() {{
    rm -rf "$temp_dir"
}}
trap cleanup EXIT INT TERM

payload_line=$(awk '/^__ARCHIVE_BELOW__/ {{ print NR + 1; exit }}' "$0")
tail -n +"$payload_line" "$0" | base64 -d | tar -xz -C "$temp_dir"

bundle_dir="$temp_dir/$env_name"
installer_path="$bundle_dir/installer"

if [ ! -x "$installer_path" ]; then
    chmod +x "$installer_path"
fi

export CONDA_DIST_BUNDLE_DIR="$bundle_dir"
export CONDA_DIST_PROJECT_NAME="$env_name"

if "$installer_path" "$@"; then
    status=0
else
    status=$?
fi

exit "$status"

__ARCHIVE_BELOW__
"#,
        env_name = environment_name
    )
}

fn write_self_extracting_script(
    script_path: &Path,
    environment_name: &str,
    archive_bytes: &[u8],
) -> Result<()> {
    if let Some(parent) = script_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create parent directory {}", parent.display())
            })?;
        }
    }

    let prologue = installer_prologue(environment_name);
    let mut file = fs::File::create(script_path)
        .with_context(|| format!("failed to create {}", script_path.display()))?;
    file.write_all(prologue.as_bytes())
        .with_context(|| format!("failed to write script header {}", script_path.display()))?;

    let encoded = STANDARD.encode(archive_bytes);
    for chunk in encoded.as_bytes().chunks(76) {
        file.write_all(chunk).with_context(|| {
            format!("failed to write archive chunk to {}", script_path.display())
        })?;
        file.write_all(b"\n")
            .with_context(|| format!("failed to finalize newline in {}", script_path.display()))?;
    }
    file.flush()
        .with_context(|| format!("failed to flush {}", script_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(script_path)
            .with_context(|| format!("failed to read permissions for {}", script_path.display()))?
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(script_path, perms).with_context(|| {
            format!(
                "failed to set executable permissions on {}",
                script_path.display()
            )
        })?;
    }

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
