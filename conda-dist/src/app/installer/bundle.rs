use std::{
    collections::{BTreeMap, HashSet},
    convert::TryFrom,
    fs,
    io::Write,
    path::Path,
    str::FromStr,
};

use anyhow::{Context, Result, bail};
use flate2::{Compression, write::GzEncoder};
use rattler_conda_types::{PackageName, Platform, RepoDataRecord};
use rattler_lock::{LockFile, LockFileBuilder};
use serde::Serialize;
use tar::{Builder, Header, HeaderMode};

use crate::{
    app::{artifact_name, environment::EnvironmentPreparation},
    conda::{self, LOCKFILE_NAME},
    config::{BundleMetadataConfig, CondaDistConfig},
    progress::ProgressCounter,
};

use super::InstallerPaths;

include!(concat!(env!("OUT_DIR"), "/installers.rs"));

const MAGIC_BYTES: &[u8] = b"CONDADIST!";

#[derive(Debug, Clone, Serialize)]
pub struct BundleMetadataManifest {
    pub summary: String,
    pub author: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_notes: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub featured_packages: Vec<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub components: BTreeMap<String, Vec<String>>,
}

impl BundleMetadataManifest {
    pub fn from_config(
        environment_name: &str,
        config: Option<&BundleMetadataConfig>,
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
                format!("featured package '{entry}' is not a valid package name")
            })?;

            if !available_names.contains(&package_name) {
                bail!("featured package '{entry}' was not found in the resolved environment");
            }

            if seen.insert(package_name.clone()) {
                featured.push(package_name.as_normalized().to_string());
            }
        }

        Ok(Self {
            summary,
            author,
            description,
            release_notes,
            featured_packages: featured,
            components: BTreeMap::new(),
        })
    }
}

pub(super) fn create_installers(
    output_dir: &Path,
    config: &CondaDistConfig,
    prep: &EnvironmentPreparation,
    progress: &mut ProgressCounter,
) -> Result<InstallerPaths> {
    fs::create_dir_all(output_dir).with_context(|| {
        format!(
            "failed to prepare installer output directory {}",
            output_dir.display()
        )
    })?;

    let mut written = BTreeMap::new();
    let mut completed = 0;
    for subset in
        std::iter::once(None).chain(config.subsets().keys().map(|name| Some(name.as_str())))
    {
        let name = artifact_name(config.name(), subset);
        let mut metadata = prep.bundle_metadata.clone();
        if let Some(subset) = subset {
            let components = config.subsets()[subset].resolved_components();
            metadata
                .components
                .retain(|name, _| components.contains(name));
        }
        let mut paths = BTreeMap::new();
        for platform in &prep.target_platforms {
            let lockfile = installer_lockfile(&prep.lockfile, *platform, &metadata)?;
            let records = conda::locked_packages(&lockfile, "default", *platform)?;
            let mut platform_metadata = metadata.clone();
            platform_metadata.featured_packages.retain(|name| {
                records
                    .iter()
                    .any(|record| record.package_record.name.as_normalized() == name)
            });
            let metadata_blob = launcher_metadata_blob(&platform_metadata)?;
            let installer_bytes =
                embedded_installer_for_platform(*platform).with_context(|| {
                    format!("no embedded installer available for platform {platform}")
                })?;
            let archive_bytes = create_tar_gz(&prep.channel_dir, &name, &lockfile, &records)
                .with_context(|| format!("failed to prepare archive for {name} on {platform}"))?;
            let target_path = output_dir.join(format!("{name}-{platform}"));
            write_self_extracting_installer(
                &target_path,
                installer_bytes,
                &metadata_blob,
                &archive_bytes,
            )
            .with_context(|| format!("failed to write installer {}", target_path.display()))?;
            paths.insert(*platform, target_path);
            completed += 1;
            progress.set(completed);
        }
        written.insert(name, paths);
    }
    Ok(written)
}

/// Keeps the base and this installer's component environments for one target platform.
/// The default environment is their union, so installing without a selection installs everything.
fn installer_lockfile(
    lockfile: &LockFile,
    platform: Platform,
    metadata: &BundleMetadataManifest,
) -> Result<LockFile> {
    let mut builder = LockFileBuilder::new();
    let mut full = BTreeMap::new();
    for name in std::iter::once("default:base".to_string()).chain(
        metadata
            .components
            .keys()
            .map(|name| format!("default:component:{name}")),
    ) {
        let environment = lockfile
            .environment(&name)
            .with_context(|| format!("missing installer environment '{name}'"))?;
        builder.set_channels(&name, ["file:./"]);
        builder.set_options(&name, environment.solve_options().clone());
        for package in environment.conda_packages(platform).into_iter().flatten() {
            builder.add_conda_package(&name, platform, package.clone());
            full.insert(package.record().name.as_normalized(), package);
        }
    }
    builder.set_channels("default", ["file:./"]);
    builder.set_options(
        "default",
        lockfile
            .default_environment()
            .context("missing installer default environment")?
            .solve_options()
            .clone(),
    );
    for package in full.into_values() {
        builder.add_conda_package("default", platform, package.clone());
    }
    Ok(builder.finish())
}

fn embedded_installer_for_platform(platform: Platform) -> Option<&'static [u8]> {
    let key = platform.as_str();
    INSTALLERS
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, bytes)| *bytes)
}

/// Archives the filtered lockfile and only its package files, including `noarch` packages.
fn create_tar_gz(
    root_dir: &Path,
    root_name: &str,
    lockfile: &LockFile,
    records: &[RepoDataRecord],
) -> Result<Vec<u8>> {
    let encoder = GzEncoder::new(Vec::new(), Compression::new(6));
    let mut builder = Builder::new(encoder);
    builder.mode(HeaderMode::Deterministic);
    builder.append_dir(root_name, root_dir)?;

    let lockfile_bytes = lockfile.render_to_string()?;
    let mut header = Header::new_gnu();
    header.set_size(lockfile_bytes.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder.append_data(
        &mut header,
        format!("{root_name}/{LOCKFILE_NAME}"),
        lockfile_bytes.as_bytes(),
    )?;
    for record in records {
        let relative = Path::new(&record.package_record.subdir).join(&record.file_name);
        let source = root_dir.join(&relative);
        builder
            .append_path_with_name(&source, Path::new(root_name).join(relative))
            .with_context(|| format!("failed to add package {} to archive", source.display()))?;
    }
    let encoder = builder
        .into_inner()
        .context("failed to finalize tar archive")?;
    encoder
        .finish()
        .context("failed to complete gzip compression")
}

fn launcher_metadata_blob(metadata: &BundleMetadataManifest) -> Result<Vec<u8>> {
    serde_json::to_vec(metadata).context("failed to encode embedded metadata")
}

fn write_self_extracting_installer(
    output_path: &Path,
    installer_bytes: &[u8],
    metadata_bytes: &[u8],
    payload_bytes: &[u8],
) -> Result<()> {
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
