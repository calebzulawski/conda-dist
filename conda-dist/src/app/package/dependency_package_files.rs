//! Resolve locked package records and their file lists for modular native packaging.

use std::path::PathBuf;

use anyhow::Result;
use rattler::install::PythonInfo;
use rattler::package_cache::PackageCache;
use rattler_conda_types::package::{LinkJson, NoArchLinks, PackageFile, PathsJson};
use rattler_conda_types::{Platform, RepoDataRecord};
use rattler_networking::LazyClient;

use crate::{
    app::{environment::EnvironmentPreparation, package::model::DependencyPackage},
    conda,
};

/// Resolve the locked packages for a platform and return their file lists.
pub async fn collect_dependencies(
    prep: &EnvironmentPreparation,
    workspace: &crate::workspace::Workspace,
    platform: Platform,
) -> Result<Vec<DependencyPackage>> {
    let selected: std::collections::BTreeMap<_, _> =
        conda::locked_packages(&prep.lockfile, "default", platform)?
            .into_iter()
            .map(|record| {
                (
                    record.package_record.name.as_normalized().to_string(),
                    record,
                )
            })
            .collect();

    let python_info = resolve_python_noarch_info(selected.values(), platform);

    let package_cache = PackageCache::new(workspace.package_cache_dir());
    let client = conda::authenticated_client()?;

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
        let link_path = cache_lock.path().join(LinkJson::package_path());
        if link_path.is_file()
            && let NoArchLinks::Python(entry_points) = LinkJson::from_path(link_path)?.noarch
        {
            files.extend(
                entry_points
                    .entry_points
                    .into_iter()
                    .map(|entry| info.bin_dir.join(entry.command)),
            );
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
