use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    pub fn from_manifest_dir(manifest_dir: &Path, explicit: Option<PathBuf>) -> Result<Self> {
        let root = match explicit {
            Some(path) => {
                if path.is_absolute() {
                    path
                } else {
                    manifest_dir.join(path)
                }
            }
            None => manifest_dir.join(".conda-dist"),
        };

        fs::create_dir_all(&root)
            .with_context(|| format!("failed to prepare workspace directory {}", root.display()))?;

        Ok(Self { root })
    }

    pub fn container_root(&self) -> PathBuf {
        self.root.join("container")
    }

    pub fn package_cache_dir(&self) -> PathBuf {
        self.root.join("package-cache")
    }

    pub fn native_packaging_dir(&self) -> PathBuf {
        self.root.join("native-packaging")
    }
}
