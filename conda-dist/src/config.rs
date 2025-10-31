use std::{collections::BTreeMap, fs, path::Path};

use anyhow::{Context, Result, bail};
use rattler_conda_types::{MatchSpec, ParseStrictness, Platform};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct CondaDistConfig {
    name: String,
    author: String,
    version: String,
    channels: Vec<String>,
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

    pub fn author(&self) -> &str {
        &self.author
    }

    pub fn version(&self) -> &str {
        &self.version
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

    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            bail!("manifest field 'name' must not be empty");
        }
        if !self.name.is_ascii() {
            bail!("manifest field 'name' must contain only ASCII characters");
        }
        if self
            .name
            .chars()
            .any(|ch| !(ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.')))
        {
            bail!("manifest field 'name' may only contain ASCII letters, digits, '-', '_', or '.'");
        }
        if self.author.trim().is_empty() {
            bail!("manifest field 'author' must not be empty");
        }
        if self.version.trim().is_empty() {
            bail!("manifest field 'version' must not be empty");
        }
        if self.version.chars().any(|ch| ch.is_whitespace()) {
            bail!("manifest field 'version' must not contain whitespace");
        }
        if !self.version.is_ascii() {
            bail!("manifest field 'version' must contain only ASCII characters");
        }
        if self.channels.is_empty() {
            bail!("manifest must contain at least one entry in 'channels'");
        }
        if self
            .channels
            .iter()
            .any(|channel| channel.trim().is_empty())
        {
            bail!("manifest 'channels' entries must not be empty");
        }
        if self.platforms.is_empty() {
            bail!("manifest must contain at least one entry in 'platforms'");
        }
        if self
            .platforms
            .iter()
            .any(|platform| platform.trim().is_empty())
        {
            bail!("manifest 'platforms' entries must not be empty");
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
pub struct DependencySpec(BTreeMap<String, String>);

impl DependencySpec {
    pub fn to_match_specs(&self) -> Result<Vec<MatchSpec>> {
        self.0
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
            .collect()
    }
}

pub fn load_manifest(path: &Path) -> Result<CondaDistConfig> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read manifest at {}", path.display()))?;
    let config: CondaDistConfig = toml::from_str(&raw)
        .with_context(|| format!("failed to parse manifest {}", path.display()))?;
    config.validate()?;
    Ok(config)
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct BundleMetadataConfig {
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub release_notes: Option<String>,
    #[serde(default)]
    pub featured_packages: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ContainerConfig {
    #[serde(default = "default_base_image")]
    pub base_image: String,
    #[serde(default)]
    pub prefix: Option<String>,
    #[serde(default = "default_tag_template")]
    pub tag_template: String,
}

impl Default for ContainerConfig {
    fn default() -> Self {
        Self {
            base_image: default_base_image(),
            prefix: None,
            tag_template: default_tag_template(),
        }
    }
}

fn default_base_image() -> String {
    "gcr.io/distroless/base-debian12".to_string()
}

fn default_tag_template() -> String {
    "{name}:{version}".to_string()
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
