//! Core package data types and naming helpers for native packaging.

use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};
use std::str::FromStr;

use rattler_conda_types::{Platform, RepoDataRecord, VersionWithSource};

use crate::app::{context::ManifestContext, environment::EnvironmentPreparation};
use crate::installer;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PackageFormat {
    Rpm,
    Deb,
}

impl PackageFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Rpm => "rpm",
            Self::Deb => "deb",
        }
    }
}

#[derive(Debug)]
pub struct PackageResult {
    pub format: PackageFormat,
    pub image: String,
    pub platform: Platform,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct DependencyPackage {
    pub record: RepoDataRecord,
    pub files: Vec<PathBuf>,
    pub extra_build: Option<String>,
}

/// Return the author string formatted for package metadata.
pub fn package_author(manifest_ctx: &ManifestContext) -> String {
    sanitize_single_line(manifest_ctx.config.author())
}

/// Resolve the install prefix used by native package payloads.
pub fn install_prefix(
    manifest_ctx: &ManifestContext,
    prep: &EnvironmentPreparation,
) -> Result<String> {
    let prefix = manifest_ctx
        .config
        .container()
        .and_then(|cfg| cfg.prefix.clone())
        .unwrap_or_else(|| format!("/opt/{}", prep.environment_name));
    if !prefix.starts_with('/') {
        bail!(
            "install prefix '{prefix}' must be an absolute path; update container.prefix or specify a fully-qualified path"
        );
    }
    Ok(prefix)
}

/// Determine the base package summary, falling back to the environment name.
pub fn base_summary(prep: &EnvironmentPreparation) -> Result<String> {
    let name = prep.environment_name.as_str();
    let summary_source = prep.bundle_metadata.summary.trim();
    let summary_line = if summary_source.is_empty() {
        name.to_string()
    } else {
        sanitize_single_line(summary_source)
    };
    if summary_line.is_empty() {
        bail!("package summary for native builds must not be empty");
    }
    Ok(summary_line)
}

/// Collect description paragraphs for the base package metadata.
pub fn base_description_parts(prep: &EnvironmentPreparation) -> Vec<String> {
    compose_description_parts(&prep.bundle_metadata)
}

/// Build the native package name for a dependency package.
pub fn split_package_name(env_name: &str, source_name: &str) -> String {
    format!("{}-{}", env_name, sanitize_native_name(source_name))
}

/// Parse the manifest version as a conda version.
pub fn base_version(manifest_ctx: &ManifestContext) -> Result<VersionWithSource> {
    VersionWithSource::from_str(manifest_ctx.config.version().trim())
        .map_err(|_| anyhow!("package version must be a valid conda version string"))
}

/// Ensure native packages are only built for supported Linux platforms.
pub fn ensure_linux_package_platform(platform: Platform) -> Result<()> {
    if platform == Platform::NoArch || !platform.as_str().starts_with("linux-") {
        bail!(
            "native package builds are only supported for linux platforms (received '{}')",
            platform.as_str()
        );
    }
    Ok(())
}

/// Normalize a string into a native package-safe name.
pub fn sanitize_native_name(raw: &str) -> String {
    let mut out = String::new();
    let mut last_sep = false;
    for ch in raw.chars() {
        let mapped = if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '+' | '-') {
            Some(ch.to_ascii_lowercase())
        } else {
            None
        };
        match mapped {
            Some(value) => {
                out.push(value);
                last_sep = false;
            }
            None => {
                if !last_sep {
                    out.push('-');
                    last_sep = true;
                }
            }
        }
    }
    let trimmed = out.trim_matches('-');
    let mut cleaned = if trimmed.is_empty() {
        "pkg".to_string()
    } else {
        trimmed.to_string()
    };
    if !cleaned
        .chars()
        .next()
        .map(|ch| ch.is_ascii_alphanumeric())
        .unwrap_or(false)
    {
        cleaned = format!("pkg-{cleaned}");
    }
    cleaned
}

fn sanitize_single_line(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn compose_description_parts(manifest: &installer::BundleMetadataManifest) -> Vec<String> {
    let mut sections = Vec::new();
    if let Some(desc) = manifest.description.as_ref() {
        let trimmed = desc.trim();
        if !trimmed.is_empty() {
            sections.push(trimmed.to_string());
        }
    }
    if let Some(notes) = manifest.release_notes.as_ref() {
        let trimmed = notes.trim();
        if !trimmed.is_empty() {
            sections.push(format!("Release notes:\n{trimmed}"));
        }
    }
    sections
}
