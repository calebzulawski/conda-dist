pub mod container;
pub mod context;
pub mod environment;
pub mod installer;
mod package;
mod runtime;

use anyhow::Result;

use crate::downloader::DownloadSummary;

use crate::cli::{Cli, Command};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockMode {
    Auto,
    Unlock,
    Locked,
}

pub async fn execute(cli: Cli) -> Result<()> {
    let Cli {
        work_dir,
        locked,
        unlock,
        command,
    } = cli;
    let lock_mode = if unlock {
        LockMode::Unlock
    } else if locked {
        LockMode::Locked
    } else {
        LockMode::Auto
    };
    match command {
        Command::Lock(args) => environment::execute_lock(args, work_dir, lock_mode).await,
        Command::Installer(args) => installer::execute(args, work_dir.clone(), lock_mode).await,
        Command::Container(args) => container::execute(args, work_dir, lock_mode).await,
        Command::Package(args) => package::execute(args, work_dir, lock_mode).await,
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
