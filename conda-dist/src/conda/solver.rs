use anyhow::{Context, Result, bail};
use rattler_conda_types::{Channel, GenericVirtualPackage, MatchSpec, Platform, RepoDataRecord};
use rattler_repodata_gateway::Gateway;
use rattler_solve::{
    ChannelPriority, RepoDataIter, SolveStrategy, SolverImpl, SolverTask, resolvo,
};

pub async fn solve_environment(
    gateway: &Gateway,
    channels: &[Channel],
    specs: &[MatchSpec],
    solve_platforms: &[Platform],
    locked_packages: Vec<RepoDataRecord>,
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
        locked_packages,
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
