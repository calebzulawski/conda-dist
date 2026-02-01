use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use rattler_conda_types::Platform;

use crate::installer::InstallerPlatformSelection;

#[derive(Debug, Parser)]
#[command(version, about = "Build distributable artifacts from Conda environments", long_about = None)]
pub struct Cli {
    /// Workspace directory used for cached artifacts (defaults to <manifest>/.conda-dist)
    #[arg(long = "work-dir", value_name = "PATH", global = true)]
    pub work_dir: Option<PathBuf>,

    /// Require the existing lockfile and skip solving; fails if the lockfile is stale or missing
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

    /// Optional directory to write the installer binary
    #[arg(long = "output-dir", alias = "output", value_name = "PATH")]
    pub output_dir: Option<PathBuf>,

    /// Select which installer platform(s) to build
    #[arg(
        long = "installer-platform",
        value_name = "PLATFORM",
        default_value = "all"
    )]
    pub installer_platform: InstallerPlatformSelection,
}

#[derive(Debug, Args)]
pub struct ContainerArgs {
    /// Path to the conda-dist manifest (conda-dist.toml)
    #[arg(value_name = "MANIFEST", default_value = "conda-dist.toml")]
    pub manifest: PathBuf,

    /// Restrict the build to a single target platform
    #[arg(long = "platform", value_name = "PLATFORM")]
    pub platform: Option<Platform>,

    /// Path to write the resulting OCI archive (defaults to <manifest-dir>/<name>-container.oci.tar)
    #[arg(long = "oci-output", value_name = "PATH")]
    pub oci_output: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct PackageArgs {
    /// Path to the conda-dist manifest (conda-dist.toml)
    #[arg(value_name = "MANIFEST", default_value = "conda-dist.toml")]
    pub manifest: PathBuf,

    /// Restrict native packaging to specific image name(s) from the manifest
    #[arg(long = "image", value_name = "NAME")]
    pub image: Vec<String>,

    /// Restrict native packaging to specific target platform(s) (defaults to host platform)
    #[arg(long = "platform", value_name = "PLATFORM")]
    pub platform: Vec<Platform>,

    /// Output directory for generated packages (defaults to <manifest-dir>)
    #[arg(long = "output-dir", alias = "output", value_name = "PATH")]
    pub output_dir: Option<PathBuf>,
}

pub fn parse() -> Cli {
    Cli::parse()
}
