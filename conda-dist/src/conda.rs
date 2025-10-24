use std::str::FromStr;

use anyhow::{Context, Result, bail};
use rattler::default_cache_dir;
use rattler_conda_types::{
    Channel, ChannelConfig, GenericVirtualPackage, MatchSpec, Platform, RepoDataRecord,
};
use rattler_lock::{CondaPackageData, LockFile, LockFileBuilder, SolveOptions};
use rattler_repodata_gateway::{Gateway, GatewayBuilder};
use rattler_solve::{
    ChannelPriority, RepoDataIter, SolveStrategy, SolverImpl, SolverTask, resolvo,
};
use rattler_virtual_packages::{VirtualPackage, VirtualPackageOverrides};

pub const DEFAULT_CHANNEL: &str = "conda-forge";
pub const LOCKFILE_NAME: &str = "conda-lock.yml";

pub fn parse_channels(channel_strings: &[String], config: &ChannelConfig) -> Result<Vec<Channel>> {
    channel_strings
        .iter()
        .map(|ch| {
            Channel::from_str(ch, config).with_context(|| format!("failed to parse channel '{ch}'"))
        })
        .collect()
}

pub fn resolve_target_platforms(raw_platforms: &[String]) -> Result<Vec<Platform>> {
    if raw_platforms.is_empty() {
        return Ok(vec![Platform::current()]);
    }

    raw_platforms
        .iter()
        .map(|value| {
            Platform::from_str(value).with_context(|| format!("failed to parse platform '{value}'"))
        })
        .collect()
}

pub fn augment_with_noarch(platforms: &[Platform]) -> Vec<Platform> {
    let mut solve_platforms = platforms.to_vec();
    if !solve_platforms
        .iter()
        .any(|platform| *platform == Platform::NoArch)
    {
        solve_platforms.push(Platform::NoArch);
    }
    solve_platforms
}

pub fn detect_virtual_packages() -> Result<Vec<GenericVirtualPackage>> {
    let overrides = VirtualPackageOverrides::default();
    let packages = VirtualPackage::detect(&overrides)?
        .into_iter()
        .map(GenericVirtualPackage::from)
        .collect();
    Ok(packages)
}

pub fn build_gateway() -> Result<Gateway> {
    let mut builder = GatewayBuilder::new();
    #[cfg(not(target_arch = "wasm32"))]
    {
        let cache_root = default_cache_dir()?.join("repodata");
        builder.set_cache_dir(&cache_root);
    }

    Ok(builder.finish())
}

pub async fn solve_environment(
    gateway: &Gateway,
    channels: &[Channel],
    specs: &[MatchSpec],
    solve_platforms: &[Platform],
    virtual_packages: Vec<GenericVirtualPackage>,
) -> Result<Vec<RepoDataRecord>> {
    let repo_data_sets = gateway
        .query(
            channels.to_vec(),
            solve_platforms.iter().copied(),
            specs.to_vec(),
        )
        .recursive(true)
        .execute()
        .await?;

    if repo_data_sets.is_empty() {
        bail!("the solver received no available packages; check your channels and platforms");
    }

    let available_packages: Vec<_> = repo_data_sets
        .iter()
        .map(|data| RepoDataIter(data.iter()))
        .collect();

    let mut solver = resolvo::Solver::default();
    let solve_result = solver.solve(SolverTask {
        available_packages,
        locked_packages: Vec::new(),
        pinned_packages: Vec::new(),
        virtual_packages,
        specs: specs.to_vec(),
        constraints: Vec::new(),
        timeout: None,
        channel_priority: ChannelPriority::Strict,
        exclude_newer: None,
        strategy: SolveStrategy::Highest,
    })?;

    let mut solved_records = solve_result.records;

    #[cfg(not(target_arch = "wasm32"))]
    {
        gateway
            .ensure_run_exports(solved_records.iter_mut(), None)
            .await
            .context("failed to enrich run exports for solved records")?;
    }

    Ok(solved_records)
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
