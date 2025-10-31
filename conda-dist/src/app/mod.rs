pub mod container;
pub mod context;
pub mod environment;
pub mod installer;
mod package;
mod runtime;

use anyhow::Result;

use crate::cli::{Cli, Command};

pub async fn execute(cli: Cli) -> Result<()> {
    let Cli { work_dir, command } = cli;
    match command {
        Command::Installer(args) => installer::execute(args, work_dir.clone()).await,
        Command::Container(args) => container::execute(args, work_dir).await,
        Command::Package(args) => package::execute(args, work_dir).await,
    }
}
