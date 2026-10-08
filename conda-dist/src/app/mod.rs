pub mod container;
pub mod context;
pub mod environment;
pub mod installer;
#[cfg(unix)]
mod package;
#[cfg(not(unix))]
mod package {
    use anyhow::{Result, bail};

    use crate::cli::PackageArgs;

    use super::LockMode;

    pub async fn execute(
        _args: PackageArgs,
        _work_dir: Option<std::path::PathBuf>,
        _engine: Option<std::path::PathBuf>,
        _engine_flags: Vec<String>,
        _lock_mode: LockMode,
    ) -> Result<()> {
        bail!("native package builds are only supported on unix platforms");
    }
}
mod runtime;

use anyhow::Result;

use crate::cli::{Cli, Command};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockMode {
    Auto,
    Unlock,
    Locked,
}

fn artifact_name(name: &str, selection: Option<&str>) -> String {
    match selection {
        Some(selection) => format!("{name}-{selection}"),
        None => name.to_string(),
    }
}

pub async fn execute(cli: Cli) -> Result<()> {
    let Cli {
        work_dir,
        locked,
        unlock,
        engine,
        engine_flags,
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
        Command::Container(args) => {
            container::execute(args, work_dir, engine, engine_flags, lock_mode).await
        }
        Command::Package(args) => {
            package::execute(args, work_dir, engine, engine_flags, lock_mode).await
        }
    }
}
