use std::{
    env,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use rattler_conda_types::Platform;
use tokio::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeEngine {
    Docker,
    Podman,
}

#[derive(Debug, Clone)]
pub struct RuntimeBinary {
    binary: PathBuf,
    engine: RuntimeEngine,
}

impl RuntimeBinary {
    pub fn new(binary: PathBuf, engine: RuntimeEngine) -> Self {
        Self { binary, engine }
    }

    pub fn binary(&self) -> &Path {
        &self.binary
    }

    pub fn engine(&self) -> RuntimeEngine {
        self.engine
    }
}

pub fn resolve_runtime(explicit: Option<PathBuf>) -> Result<RuntimeBinary> {
    if let Some(path) = explicit {
        match detect_runtime_engine(&path) {
            Some(engine) => return Ok(RuntimeBinary::new(path, engine)),
            None => bail!(
                "unable to determine engine type for {} (expected docker or podman binary)",
                path.display()
            ),
        }
    }

    if let Some(path) = find_in_path("docker") {
        return Ok(RuntimeBinary::new(path, RuntimeEngine::Docker));
    }

    if let Some(path) = find_in_path("podman") {
        return Ok(RuntimeBinary::new(path, RuntimeEngine::Podman));
    }

    bail!("no container engine found; install docker or podman, or supply --engine <path>");
}

fn detect_runtime_engine(path: &Path) -> Option<RuntimeEngine> {
    let name = path.file_name()?.to_str()?.to_lowercase();
    if name.contains("docker") {
        Some(RuntimeEngine::Docker)
    } else if name.contains("podman") {
        Some(RuntimeEngine::Podman)
    } else {
        None
    }
}

fn find_in_path(binary: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    env::split_paths(&path_var)
        .map(|dir| dir.join(binary))
        .find(|candidate| candidate.is_file())
}

pub async fn run_command(cmd: &mut Command, action: &str) -> Result<()> {
    let display = {
        let std_cmd = cmd.as_std();
        let program = std_cmd.get_program().to_string_lossy().into_owned();
        let args = std_cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        if args.is_empty() {
            program
        } else {
            format!("{} {}", program, args.join(" "))
        }
    };

    let output = cmd
        .output()
        .await
        .with_context(|| format!("failed to execute {} command", action))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!(
            "{} failed (exit code {}):\ncommand: {}\n{}\n{}",
            action,
            output
                .status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "signal".to_string()),
            display,
            stdout,
            stderr
        )
    }
}

pub fn platform_to_runtime_spec(platform: Platform) -> Result<&'static str> {
    match platform {
        Platform::Linux64 => Ok("linux/amd64"),
        Platform::LinuxAarch64 => Ok("linux/arm64/v8"),
        Platform::LinuxPpc64le => Ok("linux/ppc64le"),
        Platform::LinuxS390X => Ok("linux/s390x"),
        Platform::Linux32 => Ok("linux/386"),
        Platform::LinuxArmV7l => Ok("linux/arm/v7"),
        other => bail!(
            "unsupported platform '{}' for container runtime execution",
            other.as_str()
        ),
    }
}

pub fn format_platform_list(platforms: &[Platform]) -> String {
    let mut names: Vec<&str> = platforms.iter().map(|p| p.as_str()).collect();
    names.sort_unstable();
    names.join(", ")
}
