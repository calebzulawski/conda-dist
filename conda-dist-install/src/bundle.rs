use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use rattler_conda_types::{Platform, RepoDataRecord};
use rattler_lock::{CondaPackageData, DEFAULT_ENVIRONMENT_NAME, LockFile};
use serde::Deserialize;
use url::Url;

const DEFAULT_LOCKFILE_NAME: &str = "conda-lock.yml";
const BUNDLE_METADATA_FILE: &str = "bundle-metadata.json";

pub struct BundleData {
    pub channel_dir: PathBuf,
    pub metadata: BundleMetadata,
    pub records: Vec<RepoDataRecord>,
    pub target_platform: Platform,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BundleMetadata {
    pub display_name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub release_notes: Option<String>,
    #[serde(default)]
    pub success_message: Option<String>,
    #[serde(default)]
    pub featured_packages: Vec<FeaturedPackage>,
    #[serde(default)]
    pub post_install: Option<PostInstall>,
}

impl BundleMetadata {
    fn fallback(environment_name: &str) -> Self {
        Self {
            display_name: environment_name.to_string(),
            description: None,
            release_notes: None,
            success_message: None,
            featured_packages: Vec::new(),
            post_install: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct FeaturedPackage {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PostInstall {
    pub script: String,
}

pub fn load_bundle() -> Result<BundleData> {
    let bundle_dir = env::var("CONDA_DIST_BUNDLE_DIR").context(
        "installer bundle was not initialised correctly; please re-run the launcher script.",
    )?;

    let channel_dir = PathBuf::from(&bundle_dir)
        .canonicalize()
        .with_context(|| format!("failed to resolve installer bundle at {bundle_dir}"))?;

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
    })
}

fn locate_lockfile(channel_dir: &Path) -> Result<PathBuf> {
    let candidate = channel_dir.join(DEFAULT_LOCKFILE_NAME);
    if candidate.exists() {
        return Ok(candidate.canonicalize().with_context(|| {
            format!("failed to resolve lockfile path {}", candidate.display())
        })?);
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
