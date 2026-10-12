use std::str::FromStr;

use anyhow::{Context, Result, anyhow};
use rattler_conda_types::{Platform, RepoDataRecord};
use rattler_lock::{CondaPackageData, LockFile, LockFileBuilder, SolveOptions};
use rattler_solve::{ChannelPriority, SolveStrategy};

pub const LOCKFILE_NAME: &str = "conda-lock.yml";

pub fn locked_packages(
    lock_file: &LockFile,
    environment_name: &str,
    platform: Platform,
) -> Result<Vec<RepoDataRecord>> {
    let Some(environment) = lock_file.environment(environment_name) else {
        return Ok(Vec::new());
    };

    Ok(environment
        .conda_repodata_records(platform)
        .map_err(|err| anyhow!(err))?
        .unwrap_or_default())
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

/// Copies the project's locked environments under fixed installer names and local channels.
pub fn build_bundle_lockfile(lockfile: &LockFile, environment_name: &str) -> LockFile {
    let mut builder = LockFileBuilder::new();
    for (name, environment) in lockfile.environments() {
        let Some(suffix) = name.strip_prefix(environment_name) else {
            continue;
        };
        let name = format!("default{suffix}");
        builder.set_channels(&name, ["file:./"]);
        builder.set_options(&name, environment.solve_options().clone());
        for (platform, packages) in environment.conda_packages_by_platform() {
            for package in packages {
                builder.add_conda_package(&name, platform, package.clone());
            }
        }
    }
    builder.finish()
}
