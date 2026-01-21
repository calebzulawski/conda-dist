//! Resolve locked package records and their file lists for split native packaging.

use std::path::PathBuf;

use anyhow::Result;
use rattler::package_cache::PackageCache;
use rattler_conda_types::package::PathsJson;
use rattler_conda_types::{Platform, RepoDataRecord};
use rattler_networking::LazyClient;

use crate::{
    app::{context::ManifestContext, environment::EnvironmentPreparation},
    conda,
};

/// Resolve the locked packages for a platform and return their file lists.
pub async fn collect_dependencies(
    manifest_ctx: &ManifestContext,
    prep: &EnvironmentPreparation,
    workspace: &crate::workspace::Workspace,
    platform: Platform,
) -> Result<Vec<(RepoDataRecord, Vec<PathBuf>)>> {
    let mut solve_platforms = vec![platform];
    if platform != Platform::NoArch {
        solve_platforms.push(Platform::NoArch);
    }
    let records = conda::load_locked_packages(
        &manifest_ctx.lockfile_path(),
        &prep.environment_name,
        &solve_platforms,
    )?;

    let mut selected: std::collections::BTreeMap<String, (RepoDataRecord, bool)> =
        std::collections::BTreeMap::new();
    for record in records {
        let subdir = record.package_record.subdir.as_str();
        if subdir != platform.as_str() && subdir != Platform::NoArch.as_str() {
            continue;
        }
        let normalized = record.package_record.name.as_normalized().to_string();
        let is_platform = subdir == platform.as_str();
        match selected.get_mut(&normalized) {
            Some((existing, existing_is_platform)) => {
                if !*existing_is_platform && is_platform {
                    *existing = record;
                    *existing_is_platform = true;
                }
            }
            None => {
                selected.insert(normalized, (record, is_platform));
            }
        }
    }

    let package_cache = PackageCache::new(workspace.package_cache_dir());
    let client = LazyClient::default();

    let mut dependencies = Vec::new();
    for (_normalized, (record, _)) in selected {
        let files = package_files(&package_cache, &client, &record).await?;
        if files.is_empty() {
            continue;
        }
        dependencies.push((record, files));
    }

    Ok(dependencies)
}

/// Load the list of files from a cached conda package.
async fn package_files(
    package_cache: &PackageCache,
    client: &LazyClient,
    record: &RepoDataRecord,
) -> Result<Vec<PathBuf>> {
    let cache_lock = package_cache
        .get_or_fetch_from_url(
            &record.package_record,
            record.url.clone(),
            client.clone(),
            None,
        )
        .await?;
    let paths = PathsJson::from_package_directory_with_deprecated_fallback(cache_lock.path())?;
    Ok(paths
        .paths
        .into_iter()
        .map(|entry| entry.relative_path)
        .collect())
}
