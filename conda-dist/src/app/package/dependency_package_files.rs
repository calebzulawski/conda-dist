//! Resolve locked package records and their file lists for split native packaging.

use std::path::PathBuf;

use anyhow::Result;
use rattler::install::PythonInfo;
use rattler::package_cache::PackageCache;
use rattler_conda_types::package::PathsJson;
use rattler_conda_types::{Platform, RepoDataRecord};
use rattler_networking::LazyClient;

use crate::{
    app::{
        context::ManifestContext, environment::EnvironmentPreparation,
        package::model::DependencyPackage,
    },
    conda,
};

/// Resolve the locked packages for a platform and return their file lists.
pub async fn collect_dependencies(
    manifest_ctx: &ManifestContext,
    prep: &EnvironmentPreparation,
    workspace: &crate::workspace::Workspace,
    platform: Platform,
) -> Result<Vec<DependencyPackage>> {
    let solve_platforms = vec![platform, Platform::NoArch];
    let records = conda::load_locked_packages(
        &manifest_ctx.lockfile_path(),
        &prep.environment_name,
        &solve_platforms,
    )?;

    let mut selected: std::collections::BTreeMap<String, RepoDataRecord> =
        std::collections::BTreeMap::new();
    for record in records {
        let subdir = record.package_record.subdir.as_str();
        let normalized = record.package_record.name.as_normalized().to_string();
        let replace = subdir == platform.as_str();
        if replace || !selected.contains_key(&normalized) {
            selected.insert(normalized, record);
        }
    }

    let python_info = resolve_python_noarch_info(selected.values(), platform);

    let package_cache = PackageCache::new(workspace.package_cache_dir());
    let client = LazyClient::default();

    let mut dependencies = Vec::new();
    for (_normalized, record) in selected {
        let mut extra_build = None;
        if record.package_record.noarch.is_python()
            && let Some(info) = python_info.as_ref()
        {
            extra_build = Some(format!(
                "py{}{}",
                info.short_version.0, info.short_version.1
            ));
        }
        let files = package_files(&package_cache, &client, &record, python_info.as_ref()).await?;
        if files.is_empty() {
            continue;
        }
        dependencies.push(DependencyPackage {
            record,
            files,
            extra_build,
        });
    }

    Ok(dependencies)
}

/// Load the list of files from a cached conda package.
async fn package_files(
    package_cache: &PackageCache,
    client: &LazyClient,
    record: &RepoDataRecord,
    python_info: Option<&PythonInfo>,
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
    let mut files = paths
        .paths
        .into_iter()
        .map(|entry| entry.relative_path)
        .collect::<Vec<_>>();
    if record.package_record.noarch.is_python()
        && let Some(info) = python_info
    {
        for entry in &mut files {
            *entry = info
                .get_python_noarch_target_path(entry.as_path())
                .into_owned();
        }
    }
    Ok(files)
}

fn resolve_python_noarch_info<'a>(
    records: impl Iterator<Item = &'a RepoDataRecord>,
    platform: Platform,
) -> Option<PythonInfo> {
    let mut python_record: Option<&RepoDataRecord> = None;
    for record in records {
        if record.package_record.subdir.as_str() == platform.as_str()
            && record.package_record.name.as_normalized() == "python"
        {
            python_record = Some(record);
            break;
        }
    }

    let record = python_record?;
    PythonInfo::from_python_record(&record.package_record, platform).ok()
}
