pub mod container;
pub mod context;
pub mod environment;
pub mod installer;
mod package;
mod runtime;

use anyhow::Result;

use crate::downloader::DownloadSummary;

use crate::cli::{Cli, Command};

pub async fn execute(cli: Cli) -> Result<()> {
    let Cli { work_dir, command } = cli;
    match command {
        Command::Installer(args) => installer::execute(args, work_dir.clone()).await,
        Command::Container(args) => container::execute(args, work_dir).await,
        Command::Package(args) => package::execute(args, work_dir).await,
    }
}

pub fn push_download_summary(messages: &mut Vec<String>, summary: &DownloadSummary) {
    if summary.fetched_packages == 0 {
        messages.push("No packages required downloading.".to_string());
        return;
    }

    let reused = summary
        .total_packages
        .saturating_sub(summary.fetched_packages);
    if reused > 0 {
        messages.push(format!(
            "Downloaded {} packages (reused {}).",
            summary.fetched_packages, reused
        ));
    } else {
        messages.push(format!("Downloaded {} packages.", summary.fetched_packages));
    }
}
