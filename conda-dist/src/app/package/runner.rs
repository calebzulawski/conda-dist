//! Run containerized native packaging commands and collect outputs.

use std::{path::Path, time::SystemTime};

use anyhow::{Result, bail};
use libc::{getegid, geteuid};
use tokio::process::Command;

use super::{
    OUTPUT_DEST_PATH,
    model::{PackageFormat, PackageResult},
    plan::NativeBuild,
    runtime::{self, RuntimeBinary, RuntimeEngine},
};

const SCRIPT_DEST_PATH: &str = "/tmp/conda-dist-package.sh";
const INSTALLER_DEST_PATH: &str = "/tmp/conda-dist-installer";
const PACKAGING_DEST_PATH: &str = "/tmp/conda-dist-packages";

/// Execute a single containerized package build and collect output artifacts.
pub async fn run_package(
    runtime: &RuntimeBinary,
    prefix: &str,
    job: NativeBuild,
) -> Result<Vec<PackageResult>> {
    let mut cmd = Command::new(runtime.binary());
    cmd.arg("run").arg("--rm");
    if matches!(
        runtime.engine(),
        RuntimeEngine::Docker | RuntimeEngine::Podman
    ) {
        let spec = runtime::platform_to_runtime_spec(job.platform)?;
        cmd.arg("--platform").arg(spec);
    }
    {
        let uid = unsafe { geteuid() };
        let gid = unsafe { getegid() };
        cmd.arg("--env").arg(format!("PKG_UID={uid}"));
        cmd.arg("--env").arg(format!("PKG_GID={gid}"));
    }

    cmd.arg("--mount").arg(format!(
        "type=bind,src={},dst={},ro",
        job.script_path.display(),
        SCRIPT_DEST_PATH
    ));

    cmd.arg("--env").arg(format!("PKG_PREFIX={prefix}"));
    cmd.arg("--mount").arg(format!(
        "type=bind,src={},dst={},ro",
        job.installer_path.display(),
        INSTALLER_DEST_PATH
    ));
    cmd.arg("--env")
        .arg(format!("PKG_INSTALLER={INSTALLER_DEST_PATH}"));
    cmd.arg("--mount").arg(format!(
        "type=bind,src={},dst={}",
        job.packaging_root.display(),
        PACKAGING_DEST_PATH
    ));
    cmd.arg("--env")
        .arg(format!("PKG_PACKAGING_ROOT={PACKAGING_DEST_PATH}"));
    cmd.arg("--env")
        .arg(format!("PKG_PACKAGE_PLAN={}", job.plan_rel.display()));

    cmd.arg("--mount").arg(format!(
        "type=bind,src={},dst={}",
        job.output_dir.display(),
        OUTPUT_DEST_PATH
    ));

    cmd.arg(&job.image);
    cmd.arg("/bin/bash").arg(SCRIPT_DEST_PATH);

    let start_time = SystemTime::now();
    runtime::run_command(&mut cmd, "package build").await?;

    let output_paths = collect_new_artifacts(&job.output_dir, start_time, job.format)?;

    Ok(output_paths
        .into_iter()
        .map(|path| PackageResult {
            format: job.format,
            image: job.image.clone(),
            platform: job.platform,
            path,
        })
        .collect())
}

/// Collect newly created artifacts by extension since the packaging run started.
fn collect_new_artifacts(
    output_dir: &Path,
    start_time: SystemTime,
    format: PackageFormat,
) -> Result<Vec<std::path::PathBuf>> {
    let mut collected = Vec::new();
    let ext = match format {
        PackageFormat::Rpm => "rpm",
        PackageFormat::Deb => "deb",
    };

    if !output_dir.exists() {
        bail!(
            "container completed but output directory {} does not exist",
            output_dir.display()
        );
    }

    for entry in std::fs::read_dir(output_dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if !file_type.is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some(ext) {
            continue;
        }
        let metadata = entry.metadata()?;
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if modified >= start_time {
            collected.push(path.clone());
        }
    }

    if !collected.is_empty() {
        return Ok(collected);
    }

    bail!(
        "container completed but did not produce any new artifact in {}",
        output_dir.display()
    )
}
