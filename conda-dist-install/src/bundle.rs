use std::{
    convert::TryFrom,
    env, fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use flate2::read::GzDecoder;
use rattler_conda_types::{Platform, RepoDataRecord};
use rattler_lock::{CondaPackageData, DEFAULT_ENVIRONMENT_NAME, LockFile};
use serde::Deserialize;
use tar::Archive;
use tempfile::TempDir;
use url::Url;

const DEFAULT_LOCKFILE_NAME: &str = "conda-lock.yml";
const BUNDLE_METADATA_FILE: &str = "bundle-metadata.json";

pub struct BundleData {
    pub channel_dir: PathBuf,
    pub metadata: BundleMetadata,
    pub records: Vec<RepoDataRecord>,
    pub target_platform: Platform,
    _temp_dir: TempDir,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BundleMetadata {
    pub summary: String,
    #[serde(default = "default_author")]
    pub author: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub release_notes: Option<String>,
    #[serde(default)]
    pub featured_packages: Vec<FeaturedPackage>,
}

impl BundleMetadata {
    fn fallback(environment_name: &str) -> Self {
        Self {
            summary: environment_name.to_string(),
            author: default_author(),
            description: None,
            release_notes: None,
            featured_packages: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct FeaturedPackage {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LauncherMetadata {
    pub summary: String,
}

fn default_author() -> String {
    "unknown".to_string()
}

const MAGIC_BYTES: &[u8] = b"CONDADIST!";
const LENGTH_FIELD_SIZE: u64 = std::mem::size_of::<u64>() as u64;

struct EmbeddedLayout {
    metadata: LauncherMetadata,
    payload_len: u64,
}

pub fn read_embedded_metadata() -> Result<LauncherMetadata> {
    let exe_path = env::current_exe().context("failed to locate running installer")?;
    let mut file = fs::File::open(&exe_path)
        .with_context(|| format!("failed to open installer binary at {}", exe_path.display()))?;
    let layout = read_embedded_layout(&mut file)?;
    Ok(layout.metadata)
}

pub fn load_bundle() -> Result<BundleData> {
    let exe_path = env::current_exe().context("failed to locate running installer")?;
    let mut file = fs::File::open(&exe_path)
        .with_context(|| format!("failed to open installer binary at {}", exe_path.display()))?;
    let layout = read_embedded_layout(&mut file)?;

    let temp_dir = TempDir::new().context("failed to prepare temporary bundle directory")?;
    let payload_reader = file.take(layout.payload_len);
    let decoder = GzDecoder::new(payload_reader);
    let mut archive = Archive::new(decoder);
    archive
        .unpack(temp_dir.path())
        .context("failed to unpack installer payload")?;

    let bundle_root = resolve_bundle_root(temp_dir.path())?;
    let channel_dir = bundle_root.canonicalize().with_context(|| {
        format!(
            "failed to canonicalize extracted bundle directory {}",
            bundle_root.display()
        )
    })?;

    let lockfile_path = locate_lockfile(&channel_dir)?;
    let lockfile = LockFile::from_path(&lockfile_path)
        .with_context(|| format!("failed to read lockfile at {}", lockfile_path.display()))?;

    let (environment_name, environment) = resolve_environment(&lockfile, None)?;
    let target_platform = Platform::current();
    let records = collect_records(environment, target_platform, &channel_dir)?;
    let metadata = load_bundle_metadata(&channel_dir, &environment_name)?;

    Ok(BundleData {
        channel_dir,
        metadata,
        records,
        target_platform,
        _temp_dir: temp_dir,
    })
}

fn read_embedded_layout(file: &mut fs::File) -> Result<EmbeddedLayout> {
    let file_metadata = file
        .metadata()
        .context("failed to inspect installer binary metadata")?;
    let file_len = file_metadata.len();
    let magic_len = MAGIC_BYTES.len() as u64;

    let minimum_size = magic_len
        .checked_add(LENGTH_FIELD_SIZE * 2)
        .context("installer binary layout overflow")?;
    if file_len < minimum_size {
        bail!("installer payload is missing or corrupt");
    }

    let magic_start = file_len
        .checked_sub(magic_len)
        .context("installer image is too small to contain magic marker")?;
    file.seek(SeekFrom::Start(magic_start))
        .context("failed to seek to installer marker")?;
    let mut marker = vec![0u8; MAGIC_BYTES.len()];
    file.read_exact(&mut marker)
        .context("failed to read installer marker")?;
    if marker.as_slice() != MAGIC_BYTES {
        bail!("installer payload marker mismatch; the installer may be corrupted");
    }

    let payload_len_pos = magic_start
        .checked_sub(LENGTH_FIELD_SIZE)
        .context("installer payload footer is missing")?;
    file.seek(SeekFrom::Start(payload_len_pos))
        .context("failed to access installer payload length")?;
    let mut payload_len_buf = [0u8; LENGTH_FIELD_SIZE as usize];
    file.read_exact(&mut payload_len_buf)
        .context("failed to read installer payload length")?;
    let payload_len = u64::from_le_bytes(payload_len_buf);
    if payload_len == 0 {
        bail!("installer payload is empty");
    }

    let payload_start = payload_len_pos
        .checked_sub(payload_len)
        .context("installer payload length exceeds executable size")?;

    let metadata_len_pos = payload_start
        .checked_sub(LENGTH_FIELD_SIZE)
        .context("installer metadata footer is missing")?;
    file.seek(SeekFrom::Start(metadata_len_pos))
        .context("failed to access installer metadata length")?;
    let mut metadata_len_buf = [0u8; LENGTH_FIELD_SIZE as usize];
    file.read_exact(&mut metadata_len_buf)
        .context("failed to read installer metadata length")?;
    let metadata_len = u64::from_le_bytes(metadata_len_buf);
    if metadata_len == 0 {
        bail!("installer metadata is empty");
    }

    let metadata_start = metadata_len_pos
        .checked_sub(metadata_len)
        .context("installer metadata length exceeds executable size")?;
    let metadata_len_usize = usize::try_from(metadata_len)
        .context("installer metadata is too large for this platform")?;

    file.seek(SeekFrom::Start(metadata_start))
        .context("failed to access installer metadata")?;
    let mut metadata_bytes = vec![0u8; metadata_len_usize];
    file.read_exact(&mut metadata_bytes)
        .context("failed to read installer metadata")?;
    let metadata: LauncherMetadata =
        serde_json::from_slice(&metadata_bytes).context("failed to parse installer metadata")?;

    file.seek(SeekFrom::Start(payload_start))
        .context("failed to access installer payload")?;

    Ok(EmbeddedLayout {
        metadata,
        payload_len,
    })
}

fn locate_lockfile(channel_dir: &Path) -> Result<PathBuf> {
    let candidate = channel_dir.join(DEFAULT_LOCKFILE_NAME);
    if candidate.exists() {
        return candidate
            .canonicalize()
            .with_context(|| format!("failed to resolve lockfile path {}", candidate.display()));
    }

    bail!(
        "installer resources are incomplete (missing descriptor at {}). Please re-download the installer.",
        candidate.display()
    );
}

fn resolve_environment<'lock>(
    lockfile: &'lock LockFile,
    requested: Option<&str>,
) -> Result<(String, rattler_lock::Environment<'lock>)> {
    if let Some(name) = requested {
        let environment = lockfile
            .environment(name)
            .with_context(|| format!("installer definition '{name}' not found in bundle"))?;
        return Ok((name.to_string(), environment));
    }

    if let Some(environment) = lockfile.default_environment() {
        return Ok((DEFAULT_ENVIRONMENT_NAME.to_string(), environment));
    }

    let environments: Vec<_> = lockfile.environments().collect();
    match environments.as_slice() {
        [] => bail!("installer bundle did not contain any installation definitions"),
        [(name, environment)] => Ok(((*name).to_string(), *environment)),
        _ => bail!("installer bundle is ambiguous; contact the package author for assistance"),
    }
}

fn collect_records(
    environment: rattler_lock::Environment<'_>,
    target_platform: Platform,
    channel_dir: &Path,
) -> Result<Vec<RepoDataRecord>> {
    let mut records = Vec::new();
    let mut has_target_platform = false;

    for (platform, packages) in environment.conda_packages_by_platform() {
        if platform == target_platform {
            has_target_platform = true;
        }

        if platform != target_platform && platform != Platform::NoArch {
            continue;
        }

        for package in packages {
            match package {
                CondaPackageData::Binary(data) => {
                    let package_path = channel_dir
                        .join(&data.package_record.subdir)
                        .join(&data.file_name);
                    if !package_path.exists() {
                        bail!(
                            "installer component missing from bundle: {}",
                            package_path.display()
                        );
                    }

                    let url = Url::from_file_path(&package_path).map_err(|_| {
                        anyhow!(
                            "failed to prepare installer component {}",
                            package_path.display()
                        )
                    })?;

                    let record = RepoDataRecord {
                        package_record: data.package_record.clone(),
                        file_name: data.file_name.clone(),
                        url,
                        channel: Some("local".to_string()),
                    };
                    records.push(record);
                }
                CondaPackageData::Source(data) => {
                    bail!(
                        "installer component '{}' is not supported on this system",
                        data.package_record.name.as_source()
                    );
                }
            }
        }
    }

    if !has_target_platform {
        bail!(
            "the installer bundle does not include components for platform {}",
            target_platform.as_str()
        );
    }

    Ok(records)
}

fn load_bundle_metadata(channel_dir: &Path, environment_name: &str) -> Result<BundleMetadata> {
    let metadata_path = channel_dir.join(BUNDLE_METADATA_FILE);
    if !metadata_path.exists() {
        return Ok(BundleMetadata::fallback(environment_name));
    }

    let raw = fs::read_to_string(&metadata_path)
        .with_context(|| format!("failed to read bundle metadata {}", metadata_path.display()))?;
    let manifest: BundleMetadata = serde_json::from_str(&raw).with_context(|| {
        format!(
            "failed to parse bundle metadata {}",
            metadata_path.display()
        )
    })?;
    Ok(manifest)
}

fn resolve_bundle_root(temp_parent: &Path) -> Result<PathBuf> {
    let mut dirs = Vec::new();
    for entry in fs::read_dir(temp_parent).with_context(|| {
        format!(
            "failed to inspect extracted bundle at {}",
            temp_parent.display()
        )
    })? {
        let entry = entry.with_context(|| {
            format!(
                "failed to read bundle entry within {}",
                temp_parent.display()
            )
        })?;
        let file_type = entry.file_type().with_context(|| {
            format!(
                "failed to inspect bundle entry type for {}",
                entry.path().display()
            )
        })?;
        if file_type.is_dir() {
            dirs.push(entry.path());
        }
    }

    match dirs.len() {
        1 => Ok(dirs.remove(0)),
        0 => bail!("installer payload did not contain bundle contents"),
        _ => bail!("installer payload contained multiple bundle roots; aborting"),
    }
}
