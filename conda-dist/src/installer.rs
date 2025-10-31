use std::{
    collections::HashSet,
    convert::TryFrom,
    fs,
    io::{Cursor, Write},
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, Result, bail};
use flate2::{Compression, write::GzEncoder};
use rattler_conda_types::{PackageName, Platform, RepoDataRecord};
use serde::Serialize;
use tar::{Builder, EntryType, Header, HeaderMode};

use indicatif::ProgressBar;

use crate::{conda::LOCKFILE_NAME, config::BundleMetadataConfig};

include!(concat!(env!("OUT_DIR"), "/installers.rs"));

const BUNDLE_METADATA_FILE: &str = "bundle-metadata.json";
const MAGIC_BYTES: &[u8] = b"CONDADIST!";

#[derive(Serialize)]
struct LauncherMetadata {
    summary: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BundleMetadataManifest {
    pub summary: String,
    pub author: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_notes: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub featured_packages: Vec<FeaturedPackageManifest>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FeaturedPackageManifest {
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct PreparedBundleMetadata {
    pub manifest: BundleMetadataManifest,
}

impl PreparedBundleMetadata {
    pub fn from_config(
        environment_name: &str,
        config: Option<&BundleMetadataConfig>,
        _manifest_dir: &Path,
        records: &[RepoDataRecord],
        author: &str,
    ) -> Result<Self> {
        let config = config.cloned().unwrap_or_default();
        let BundleMetadataConfig {
            summary,
            description,
            release_notes,
            featured_packages,
        } = config;

        let summary = summary.unwrap_or_else(|| environment_name.to_string());
        let author = author.trim();
        debug_assert!(!author.is_empty(), "author validated earlier");
        let author = author.to_string();

        let available_names: HashSet<PackageName> = records
            .iter()
            .map(|record| record.package_record.name.clone())
            .collect();

        let mut seen = HashSet::new();
        let mut featured = Vec::new();
        for entry in featured_packages {
            let package_name = PackageName::from_str(&entry).with_context(|| {
                format!("featured package '{}' is not a valid package name", entry)
            })?;

            if !available_names.contains(&package_name) {
                bail!(
                    "featured package '{}' was not found in the resolved environment",
                    entry
                );
            }

            if seen.insert(package_name.clone()) {
                featured.push(FeaturedPackageManifest {
                    name: package_name.as_normalized().to_string(),
                });
            }
        }

        let manifest = BundleMetadataManifest {
            summary,
            author,
            description,
            release_notes,
            featured_packages: featured,
        };

        Ok(Self { manifest })
    }
}

#[derive(Debug, Clone)]
pub enum InstallerPlatformSelection {
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

pub fn resolve_script_path(requested: PathBuf, environment_name: &str) -> Result<PathBuf> {
    if requested.exists() {
        let metadata = fs::metadata(&requested).with_context(|| {
            format!(
                "failed to inspect requested output path {}",
                requested.display()
            )
        })?;
        if metadata.is_dir() {
            return Ok(requested.join(environment_name));
        }
    }

    Ok(requested)
}

pub fn resolve_installer_platforms(
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

pub fn create_installers(
    script_path: &Path,
    environment_name: &str,
    channel_dir: &Path,
    selected_platforms: &[Platform],
    metadata: &PreparedBundleMetadata,
    progress: &ProgressBar,
) -> Result<Vec<PathBuf>> {
    let (output_dir, name_prefix) = installer_output_spec(script_path, environment_name)?;

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

    let total = selected_platforms.len();
    if total == 0 {
        progress.set_message("Create installers (0/0)");
        progress.tick();
        return Ok(Vec::new());
    }

    progress.set_message(format!("Create installers (0/{total})"));
    progress.tick();

    let mut written = Vec::new();
    let metadata_blob = launcher_metadata_blob(metadata)?;
    for (index, platform) in selected_platforms.iter().enumerate() {
        let installer_bytes = embedded_installer_for_platform(*platform).with_context(|| {
            format!(
                "no embedded installer available for platform {}",
                platform.as_str()
            )
        })?;

        let archive_bytes = create_tar_gz_for_platform(
            channel_dir,
            environment_name,
            *platform,
            installer_bytes,
            metadata,
        )
        .with_context(|| {
            format!(
                "failed to prepare archive for platform {}",
                platform.as_str()
            )
        })?;

        let installer_name = format!("{name_prefix}-{}", platform.as_str());
        let target_path = output_dir.join(installer_name);
        write_self_extracting_installer(
            &target_path,
            installer_bytes,
            &metadata_blob,
            &archive_bytes,
        )
        .with_context(|| format!("failed to write installer {}", target_path.display()))?;
        written.push(target_path);

        let done = index + 1;
        progress.set_message(format!("Create installers ({done}/{total})"));
        progress.tick();
    }

    Ok(written)
}

fn embedded_installer_for_platform(platform: Platform) -> Option<&'static [u8]> {
    let key = platform.as_str();
    INSTALLERS
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, bytes)| *bytes)
}

fn installer_output_spec(script_path: &Path, environment_name: &str) -> Result<(PathBuf, String)> {
    if script_path.is_dir() {
        return Ok((script_path.to_path_buf(), environment_name.to_string()));
    }

    let prefix = script_path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
        .unwrap_or_else(|| environment_name.to_string());

    let output_dir = script_path
        .parent()
        .map(|parent| {
            if parent.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                parent.to_path_buf()
            }
        })
        .unwrap_or_else(|| PathBuf::from("."));

    Ok((output_dir, prefix))
}

fn create_tar_gz_for_platform(
    root_dir: &Path,
    root_name: &str,
    platform: Platform,
    installer_bytes: &[u8],
    metadata: &PreparedBundleMetadata,
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

    let metadata_bytes =
        serde_json::to_vec(&metadata.manifest).context("failed to serialize bundle metadata")?;
    append_regular_file(
        &mut builder,
        format!("{root_name}/{BUNDLE_METADATA_FILE}"),
        metadata_bytes.as_slice(),
        0o644,
    )?;

    append_regular_file(
        &mut builder,
        format!("{root_name}/installer"),
        installer_bytes,
        0o755,
    )?;

    let encoder = builder
        .into_inner()
        .context("failed to finalize tar archive")?;
    let archive = encoder
        .finish()
        .context("failed to complete gzip compression")?;
    Ok(archive)
}

fn append_regular_file<W: Write>(
    builder: &mut Builder<W>,
    path: String,
    bytes: &[u8],
    mode: u32,
) -> Result<()> {
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Regular);
    header.set_mode(mode);
    header.set_size(bytes.len() as u64);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_cksum();
    let mut cursor = Cursor::new(bytes);
    builder
        .append_data(&mut header, &path, &mut cursor)
        .with_context(|| format!("failed to add {} to archive", path))?;
    Ok(())
}

fn launcher_metadata_blob(metadata: &PreparedBundleMetadata) -> Result<Vec<u8>> {
    let launcher_metadata = LauncherMetadata {
        summary: metadata.manifest.summary.clone(),
    };
    serde_json::to_vec(&launcher_metadata).context("failed to encode launcher metadata")
}

fn write_self_extracting_installer(
    output_path: &Path,
    installer_bytes: &[u8],
    metadata_bytes: &[u8],
    payload_bytes: &[u8],
) -> Result<()> {
    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create parent directory {}", parent.display())
            })?;
        }
    }

    let mut file = fs::File::create(output_path)
        .with_context(|| format!("failed to create {}", output_path.display()))?;
    file.write_all(installer_bytes)
        .with_context(|| format!("failed to write installer stub {}", output_path.display()))?;

    let metadata_len =
        u64::try_from(metadata_bytes.len()).context("installer metadata is too large to encode")?;
    let payload_len =
        u64::try_from(payload_bytes.len()).context("installer payload is too large to encode")?;

    file.write_all(metadata_bytes).with_context(|| {
        format!(
            "failed to write installer metadata to {}",
            output_path.display()
        )
    })?;
    file.write_all(&metadata_len.to_le_bytes())
        .with_context(|| {
            format!(
                "failed to finalize metadata length in {}",
                output_path.display()
            )
        })?;
    file.write_all(payload_bytes).with_context(|| {
        format!(
            "failed to write archive payload to {}",
            output_path.display()
        )
    })?;
    file.write_all(&payload_len.to_le_bytes())
        .with_context(|| {
            format!(
                "failed to finalize installer payload size in {}",
                output_path.display()
            )
        })?;
    file.write_all(MAGIC_BYTES).with_context(|| {
        format!(
            "failed to write installer marker to {}",
            output_path.display()
        )
    })?;
    file.flush()
        .with_context(|| format!("failed to flush {}", output_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(output_path)
            .with_context(|| format!("failed to read permissions for {}", output_path.display()))?
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(output_path, perms).with_context(|| {
            format!(
                "failed to set executable permissions on {}",
                output_path.display()
            )
        })?;
    }

    Ok(())
}
