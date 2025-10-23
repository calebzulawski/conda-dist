use std::path::PathBuf;

use clap::Parser;

use crate::installer::InstallerPlatformSelection;

#[derive(Debug, Parser)]
#[command(version, about = "Solve conda dependencies and produce a lockfile", long_about = None)]
pub struct Cli {
    /// Path to the conda-dist manifest (conda-dist.toml)
    #[arg(value_name = "MANIFEST", default_value = "conda-dist.toml")]
    pub manifest: PathBuf,

    /// Optional path to write the self-extracting installer script
    #[arg(long = "output", alias = "output-dir", value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// Select which installer platform(s) to build
    #[arg(
        long = "installer-platform",
        value_name = "PLATFORM",
        default_value = "all"
    )]
    pub installer_platform: InstallerPlatformSelection,
}

pub fn parse() -> Cli {
    Cli::parse()
}
