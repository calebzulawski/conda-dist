use std::str::FromStr;

use anyhow::{Context, Result};
use rattler_conda_types::Platform;

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
    if !solve_platforms.contains(&Platform::NoArch) {
        solve_platforms.push(Platform::NoArch);
    }
    solve_platforms
}
