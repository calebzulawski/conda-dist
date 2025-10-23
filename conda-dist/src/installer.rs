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
use rattler_conda_types::Platform;
use tar::{Builder, EntryType, Header, HeaderMode};

use crate::conda::LOCKFILE_NAME;

include!(concat!(env!("OUT_DIR"), "/installers.rs"));

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
