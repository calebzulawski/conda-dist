use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::installer::InstallerPlatformSelection;

#[derive(Debug, Parser)]
#[command(version, about = "Solve conda dependencies and produce artifacts", long_about = None)]
pub struct Cli {
    /// Workspace directory used for cached artifacts (defaults to <manifest>/.conda-dist)
    #[arg(long = "work-dir", value_name = "PATH")]
    pub work_dir: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Build self-extracting installers
    Installer(InstallerArgs),
    /// Build container images embedding the environment
    Container(ContainerArgs),
}

#[derive(Debug, Args)]
pub struct InstallerArgs {
    /// Path to the conda-dist manifest (conda-dist.toml)
    #[arg(value_name = "MANIFEST", default_value = "conda-dist.toml")]
    pub manifest: PathBuf,

    /// Optional path to write the self-extracting installer script
    #[arg(long = "output", alias = "output-dir", value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// Regenerate the lockfile instead of reusing any cached version
    #[arg(long = "unlock")]
    pub unlock: bool,

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
    pub platform: Option<String>,

    /// Path to the container engine binary (defaults to docker, then podman)
    #[arg(long = "engine", value_name = "PATH")]
    pub engine: Option<PathBuf>,

    /// Regenerate the lockfile instead of reusing any cached version
    #[arg(long = "unlock")]
    pub unlock: bool,
}

pub fn parse() -> Cli {
    Cli::parse()
}
