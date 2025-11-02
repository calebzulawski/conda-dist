use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config;

#[derive(Debug)]
pub struct ManifestContext {
    pub manifest_path: PathBuf,
    pub manifest_dir: PathBuf,
    pub config: config::CondaDistConfig,
}

impl ManifestContext {
    pub fn lockfile_path(&self) -> PathBuf {
        self.manifest_path.with_extension("lock")
    }
}

pub fn canonicalize_manifest(manifest: PathBuf) -> Result<PathBuf> {
    let display = manifest.display().to_string();
    manifest
        .canonicalize()
        .with_context(|| format!("failed to resolve manifest path {display}"))
}

pub fn load_manifest_context(manifest: PathBuf) -> Result<ManifestContext> {
    let manifest_path = canonicalize_manifest(manifest)?;
    let manifest_dir = manifest_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let config = config::load_manifest(&manifest_path)?;

    Ok(ManifestContext {
        manifest_path,
        manifest_dir,
        config,
    })
}
