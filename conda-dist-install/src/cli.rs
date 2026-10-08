use std::path::PathBuf;

use anyhow::Result;
use clap::{FromArgMatches, Parser};

#[derive(Debug, Parser)]
#[command(version, disable_help_subcommand = true)]
pub struct Cli {
    /// Destination path for the installation (must not already exist)
    #[arg(
        value_name = "INSTALLATION_PATH",
        required_unless_present_any = ["summary", "list_packages", "list_packages_json", "list_components"]
    )]
    pub prefix: Option<PathBuf>,

    /// Show a bundle summary and exit
    #[arg(long)]
    pub summary: bool,

    /// List all packages contained in the bundle (human-readable) and exit
    #[arg(long = "list-packages", conflicts_with = "list_packages_json")]
    pub list_packages: bool,

    /// List all packages contained in the bundle as JSON and exit
    #[arg(long = "list-packages-json")]
    pub list_packages_json: bool,

    /// List available components
    #[arg(long)]
    pub list_components: bool,

    /// Comma-separated components to install, or 'all' (default) or 'none'
    #[arg(long, value_name = "NAMES|all|none", value_delimiter = ',')]
    pub components: Option<Vec<String>>,
}

pub fn parse(project_name: &str) -> Result<Cli> {
    use clap::CommandFactory;

    let mut command = Cli::command();
    let about = format!("Install {project_name}");
    let about: &'static str = Box::leak(about.into_boxed_str());
    command = command.about(about);

    let matches = match command.try_get_matches() {
        Ok(matches) => matches,
        Err(err) => err.exit(),
    };

    match Cli::from_arg_matches(&matches) {
        Ok(cli) => Ok(cli),
        Err(err) => err.exit(),
    }
}
