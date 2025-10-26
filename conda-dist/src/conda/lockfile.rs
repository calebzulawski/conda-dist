use std::{path::Path, str::FromStr};

use anyhow::{Context, Result, anyhow};
use rattler_conda_types::{Platform, RepoDataRecord};
use rattler_lock::{CondaPackageData, LockFile, LockFileBuilder, SolveOptions};
use rattler_solve::{ChannelPriority, SolveStrategy};

pub const LOCKFILE_NAME: &str = "conda-lock.yml";

pub fn load_locked_packages(
    path: &Path,
    environment_name: &str,
    platforms: &[Platform],
) -> Result<Vec<RepoDataRecord>> {
    let lock_file = LockFile::from_path(path)
        .with_context(|| format!("failed to load lockfile from {}", path.display()))?;
    let Some(environment) = lock_file.environment(environment_name) else {
        return Ok(Vec::new());
    };

    let mut locked_packages = Vec::new();
    for platform in platforms {
        match environment
            .conda_repodata_records(*platform)
            .map_err(|err| anyhow!(err))?
        {
            Some(records) => locked_packages.extend(records),
            None => continue,
        }
    }

    Ok(locked_packages)
}

pub fn build_lockfile(
    environment_name: &str,
    channel_urls: &[String],
    records: &[RepoDataRecord],
) -> Result<LockFile> {
    let mut builder = LockFileBuilder::new();
    builder.set_channels(environment_name, channel_urls.iter().map(String::as_str));
    builder.set_options(
        environment_name,
        SolveOptions {
            strategy: SolveStrategy::Highest,
            channel_priority: ChannelPriority::Strict,
            exclude_newer: None,
        },
    );

    for record in records {
        let platform = Platform::from_str(&record.package_record.subdir).with_context(|| {
            format!(
                "failed to parse platform '{}'",
                record.package_record.subdir
            )
        })?;
        let package = CondaPackageData::from(record.clone());
        builder.add_conda_package(environment_name, platform, package);
    }

    Ok(builder.finish())
}
