use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use rattler_conda_types::Platform;

#[derive(Debug, Parser)]
#[command(version, about = "Build distributable artifacts from Conda environments", long_about = None)]
pub struct Cli {
    /// Workspace directory used for cached artifacts (defaults to .conda-dist beside the manifest)
    #[arg(long = "work-dir", value_name = "PATH", global = true)]
    pub work_dir: Option<PathBuf>,

    /// Require the existing lockfile; fails if the lockfile is stale or missing
    #[arg(long = "locked", global = true, conflicts_with = "unlock")]
    pub locked: bool,

    /// Regenerate the lockfile even if one already exists
    #[arg(long = "unlock", global = true, conflicts_with = "locked")]
    pub unlock: bool,

    /// Path to the container engine binary (defaults to docker, then podman)
    #[arg(long = "engine", value_name = "PATH", global = true)]
    pub engine: Option<PathBuf>,

    /// Extra flags passed directly to the container engine (repeatable)
    #[arg(
        long = "engine-flag",
        alias = "engine-arg",
        value_name = "FLAG",
        global = true,
        allow_hyphen_values = true
    )]
    pub engine_flags: Vec<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Update or validate the lockfile without producing artifacts
    Lock(LockArgs),
    /// Build self-extracting installers
    Installer(InstallerArgs),
    /// Build container images embedding the environment
    Container(ContainerArgs),
    /// Build native system packages (rpm/deb) using containerized installers
    Package(PackageArgs),
}

#[derive(Debug, Args)]
pub struct LockArgs {
    /// Path to the conda-dist manifest (conda-dist.toml)
    #[arg(value_name = "MANIFEST", default_value = "conda-dist.toml")]
    pub manifest: PathBuf,
}

#[derive(Debug, Args)]
pub struct InstallerArgs {
    /// Path to the conda-dist manifest (conda-dist.toml)
    #[arg(value_name = "MANIFEST", default_value = "conda-dist.toml")]
    pub manifest: PathBuf,

    /// Directory to write installers (defaults to the manifest directory)
    #[arg(long = "output-dir", alias = "output", value_name = "PATH")]
    pub output_dir: Option<PathBuf>,

    /// Select installer platforms: all, host, or a target platform
    #[arg(
        long = "installer-platform",
        value_name = "PLATFORM",
        default_value = "all",
        value_parser = parse_installer_platforms
    )]
    // Treat the resolved platform list as one CLI argument value.
    pub installer_platform: std::vec::Vec<Platform>,
}

fn parse_installer_platforms(value: &str) -> Result<Vec<Platform>, String> {
    let value = value.trim();
    match value.to_ascii_lowercase().as_str() {
        "all" => Ok(Platform::all().collect()),
        "host" => Ok(vec![Platform::current()]),
        _ => value
            .parse::<Platform>()
            .map(|platform| vec![platform])
            .map_err(|error| error.to_string()),
    }
}

#[derive(Debug, Args)]
pub struct ContainerArgs {
    /// Path to the conda-dist manifest (conda-dist.toml)
    #[arg(value_name = "MANIFEST", default_value = "conda-dist.toml")]
    pub manifest: PathBuf,

    /// Restrict the build to a single target platform
    #[arg(long = "platform", value_name = "PLATFORM")]
    pub platform: Option<Platform>,
}

#[derive(Debug, Args)]
pub struct PackageArgs {
    /// Path to the conda-dist manifest (conda-dist.toml)
    #[arg(value_name = "MANIFEST", default_value = "conda-dist.toml")]
    pub manifest: PathBuf,

    /// Restrict native packaging to specific image name(s) from the manifest
    #[arg(long = "image", value_name = "NAME")]
    pub image: Vec<String>,

    /// Restrict native packaging to specific Linux target platform(s) (defaults to host platform)
    #[arg(
        long = "platform",
        value_name = "PLATFORM",
        default_values_t = [Platform::current()],
        value_parser = parse_package_platform
    )]
    pub platform: Vec<Platform>,

    /// Output directory for generated packages (defaults to the manifest directory)
    #[arg(long = "output-dir", alias = "output", value_name = "PATH")]
    pub output_dir: Option<PathBuf>,
}

fn parse_package_platform(value: &str) -> Result<Platform, String> {
    let platform = value.parse::<Platform>().map_err(|err| err.to_string())?;
    if !platform.as_str().starts_with("linux-") {
        return Err(format!(
            "native package builds are only supported for linux platforms (received '{platform}')"
        ));
    }
    Ok(platform)
}

pub fn parse() -> Cli {
    Cli::parse()
}
