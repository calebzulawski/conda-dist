use std::{collections::BTreeMap, fs, path::Path};

use anyhow::{Context, Result};
use rattler_conda_types::{MatchSpec, ParseStrictness, Platform};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct CondaDistConfig {
    name: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    channels: Vec<String>,
    #[serde(default)]
    platforms: Vec<String>,
    dependencies: DependencySpec,
    #[serde(default)]
    metadata: Option<BundleMetadataConfig>,
    #[serde(default)]
    container: Option<ContainerConfig>,
    #[serde(default)]
    virtual_packages: Option<VirtualPackagesConfig>,
}

impl CondaDistConfig {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    pub fn channels(&self) -> &[String] {
        &self.channels
    }

    pub fn platforms(&self) -> &[String] {
        &self.platforms
    }

    pub fn dependencies(&self) -> &DependencySpec {
        &self.dependencies
    }

    pub fn metadata(&self) -> Option<&BundleMetadataConfig> {
        self.metadata.as_ref()
    }

    pub fn container(&self) -> Option<&ContainerConfig> {
        self.container.as_ref()
    }

    pub fn virtual_packages(&self) -> Option<&VirtualPackagesConfig> {
        self.virtual_packages.as_ref()
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum DependencySpec {
    Table(BTreeMap<String, String>),
    List(Vec<String>),
}

impl DependencySpec {
    pub fn to_match_specs(&self) -> Result<Vec<MatchSpec>> {
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

pub fn load_manifest(path: &Path) -> Result<CondaDistConfig> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read manifest at {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("failed to parse manifest {}", path.display()))
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct BundleMetadataConfig {
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub release_notes: Option<String>,
    #[serde(default)]
    pub success_message: Option<String>,
    #[serde(default)]
    pub featured_packages: Vec<String>,
    #[serde(default)]
    pub post_install_script: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ContainerConfig {
    #[serde(default = "default_base_image")]
    pub base_image: String,
    #[serde(default = "default_builder_image")]
    pub builder_image: String,
    #[serde(default)]
    pub prefix: Option<String>,
    #[serde(default)]
    pub registry: Option<String>,
    #[serde(default)]
    pub organization: Option<String>,
}

impl Default for ContainerConfig {
    fn default() -> Self {
        Self {
            base_image: default_base_image(),
            builder_image: default_builder_image(),
            prefix: None,
            registry: None,
            organization: None,
        }
    }
}

fn default_base_image() -> String {
    "gcr.io/distroless/base-debian12".to_string()
}

fn default_builder_image() -> String {
    "docker.io/library/debian:bookworm-slim".to_string()
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct VirtualPackagesConfig {
    #[serde(flatten)]
    platforms: BTreeMap<String, PlatformVirtualPackageConfig>,
}

impl VirtualPackagesConfig {
    pub fn for_platform(&self, platform: Platform) -> Option<&PlatformVirtualPackageConfig> {
        self.platforms
            .get(platform.as_str())
            .or_else(|| self.platforms.get("default"))
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct PlatformVirtualPackageConfig {
    #[serde(default)]
    pub linux: Option<String>,
    #[serde(default)]
    pub osx: Option<String>,
    #[serde(default)]
    pub win: Option<String>,
    #[serde(default)]
    pub libc: Option<VirtualPackageLibcConfig>,
    #[serde(default)]
    pub cuda: Option<String>,
    #[serde(default)]
    pub archspec: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VirtualPackageLibcConfig {
    #[serde(default = "default_libc_family")]
    pub family: String,
    pub version: String,
}

fn default_libc_family() -> String {
    "glibc".to_string()
}
