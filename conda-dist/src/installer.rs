use std::{
    collections::HashSet,
    fs,
    io::{Cursor, Write},
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use flate2::{Compression, write::GzEncoder};
use rattler_conda_types::{PackageName, Platform, RepoDataRecord};
use serde::Serialize;
use tar::{Builder, EntryType, Header, HeaderMode};

use crate::{conda::LOCKFILE_NAME, config::BundleMetadataConfig};

include!(concat!(env!("OUT_DIR"), "/installers.rs"));

const BUNDLE_METADATA_FILE: &str = "bundle-metadata.json";
const POST_INSTALL_SCRIPT_NAME: &str = "post-install.sh";

#[derive(Debug, Clone, Serialize)]
pub struct BundleMetadataManifest {
    pub display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success_message: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub featured_packages: Vec<FeaturedPackageManifest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_install: Option<PostInstallManifest>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FeaturedPackageManifest {
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PostInstallManifest {
    pub script: String,
}

#[derive(Debug, Clone)]
pub struct PostInstallArtifact {
    pub file_name: String,
    pub bytes: Vec<u8>,
    pub executable: bool,
}

#[derive(Debug, Clone)]
pub struct PreparedBundleMetadata {
    pub manifest: BundleMetadataManifest,
    pub post_install: Option<PostInstallArtifact>,
}

impl PreparedBundleMetadata {
    pub fn from_config(
        environment_name: &str,
        config: Option<&BundleMetadataConfig>,
        manifest_dir: &Path,
        records: &[RepoDataRecord],
    ) -> Result<Self> {
        let config = config.cloned().unwrap_or_default();
        let BundleMetadataConfig {
            display_name,
            description,
            release_notes,
            success_message,
            featured_packages,
            post_install_script,
        } = config;

        let display_name = display_name.unwrap_or_else(|| environment_name.to_string());

        let available_names: HashSet<PackageName> = records
            .iter()
            .map(|record| record.package_record.name.clone())
            .collect();

        let mut seen = HashSet::new();
        let mut featured = Vec::new();
        for entry in featured_packages {
            let package_name = PackageName::from_str(&entry.name).with_context(|| {
                format!(
                    "featured package '{}' is not a valid package name",
                    entry.name
                )
            })?;

            if !available_names.contains(&package_name) {
                bail!(
                    "featured package '{}' was not found in the resolved environment",
                    entry.name
                );
            }

            if seen.insert(package_name.clone()) {
                featured.push(FeaturedPackageManifest {
                    name: package_name.as_normalized().to_string(),
                });
            }
        }

        let post_install = if let Some(script_path) = post_install_script {
            let resolved = manifest_dir.join(&script_path);
            let script_bytes = fs::read(&resolved).with_context(|| {
                format!(
                    "failed to read post-install script at {}",
                    resolved.display()
                )
            })?;

            Some(PostInstallArtifact {
                file_name: POST_INSTALL_SCRIPT_NAME.to_string(),
                bytes: script_bytes,
                executable: true,
            })
        } else {
            None
        };

        let manifest = BundleMetadataManifest {
            display_name,
            description,
            release_notes,
            success_message,
            featured_packages: featured,
            post_install: post_install.as_ref().map(|artifact| PostInstallManifest {
                script: artifact.file_name.clone(),
            }),
        };

        Ok(Self {
            manifest,
            post_install,
        })
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
            return Ok(requested.join(format!("{environment_name}.sh")));
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

pub fn emit_installers(
    environment_name: &str,
    script_path: &Path,
    channel_dir: &Path,
    selected_platforms: &[Platform],
    metadata: &PreparedBundleMetadata,
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

fn embedded_installer_for_platform(platform: Platform) -> Option<&'static [u8]> {
    let key = platform.as_str();
    INSTALLERS
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, bytes)| *bytes)
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
    let mut metadata_header = Header::new_gnu();
    metadata_header.set_entry_type(EntryType::Regular);
    metadata_header.set_mode(0o644);
    metadata_header.set_size(metadata_bytes.len() as u64);
    metadata_header.set_uid(0);
    metadata_header.set_gid(0);
    metadata_header.set_mtime(0);
    metadata_header.set_cksum();
    let mut metadata_cursor = Cursor::new(metadata_bytes.as_slice());
    builder.append_data(
        &mut metadata_header,
        format!("{root_name}/{BUNDLE_METADATA_FILE}"),
        &mut metadata_cursor,
    )?;

    if let Some(script) = &metadata.post_install {
        let mut script_header = Header::new_gnu();
        script_header.set_entry_type(EntryType::Regular);
        script_header.set_mode(if script.executable { 0o755 } else { 0o644 });
        script_header.set_size(script.bytes.len() as u64);
        script_header.set_uid(0);
        script_header.set_gid(0);
        script_header.set_mtime(0);
        script_header.set_cksum();
        let mut script_cursor = Cursor::new(script.bytes.as_slice());
        builder.append_data(
            &mut script_header,
            format!("{root_name}/{}", script.file_name),
            &mut script_cursor,
        )?;
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
