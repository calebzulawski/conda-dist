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

    /// Path to the container engine binary (defaults to docker, then podman)
    #[arg(long = "engine", value_name = "PATH")]
    pub engine: Option<PathBuf>,

    /// Path to write the resulting OCI archive (defaults to <manifest-dir>/<name>-container.oci.tar)
    #[arg(long = "oci-output", value_name = "PATH")]
    pub oci_output: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct PackageArgs {
    /// Path to the conda-dist manifest (conda-dist.toml)
    #[arg(value_name = "MANIFEST", default_value = "conda-dist.toml")]
    pub manifest: PathBuf,

    /// Path to the container engine binary (defaults to docker, then podman)
    #[arg(long = "engine", value_name = "PATH")]
    pub engine: Option<PathBuf>,

    /// Build RPM packages using the specified container image (repeatable)
    #[arg(long = "rpm-image", value_name = "IMAGE")]
    pub rpm_images: Vec<String>,

    /// Build DEB packages using the specified container image (repeatable)
    #[arg(long = "deb-image", value_name = "IMAGE")]
    pub deb_images: Vec<String>,

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
