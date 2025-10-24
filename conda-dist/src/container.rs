use std::{
    fs,
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use rattler_conda_types::Platform;
use tokio::process::Command;

use crate::{
    app::{EnvironmentPreparation, load_manifest_context, prepare_environment},
    cli::ContainerArgs,
    config::ContainerConfig,
    installer,
};

const INSTALLER_FILENAME: &str = "installer.sh";

pub async fn execute(args: ContainerArgs) -> Result<()> {
    let ContainerArgs {
        manifest,
        platform,
        oci_dir,
        oci_archive,
        docker,
        podman,
    } = args;

    let manifest_ctx = load_manifest_context(manifest)?;
    let container_cfg = manifest_ctx.config.container().cloned().unwrap_or_default();

    let target_platform = resolve_target_platform(&manifest_ctx, platform.as_deref())?;
    ensure_linux_platform(target_platform)?;

    let runtime = select_runtime(docker, podman)?;
    validate_output_options(&runtime, oci_dir.as_ref(), oci_archive.as_ref())?;

    let multi = MultiProgress::with_draw_target(ProgressDrawTarget::stdout());
    let step_style = ProgressStyle::with_template("{prefix} {msg}")
        .unwrap_or_else(|_| ProgressStyle::default_spinner());

    let (prep, _) =
        prepare_environment(&manifest_ctx, vec![target_platform], &multi, &step_style).await?;

    let installer_path =
        prepare_self_extracting_installer(&multi, &step_style, &prep, target_platform).await?;

    let install_prefix = container_cfg
        .prefix
        .clone()
        .unwrap_or_else(|| format!("/opt/{}", prep.environment_name));
    if !install_prefix.starts_with('/') {
        bail!(
            "container prefix '{}' must be an absolute path",
            install_prefix
        );
    }

    let context_dir = prepare_build_directory(&manifest_ctx.manifest_dir, &prep.environment_name)?;
    let build_context = create_build_context(
        &context_dir,
        &installer_path,
        &container_cfg,
        &install_prefix,
        &prep.environment_name,
    )?;

    build_image(&runtime, &build_context, target_platform).await?;

    export_optional_artifacts(
        &runtime,
        oci_dir.as_ref(),
        oci_archive.as_ref(),
        &runtime.tag,
    )
    .await?;

    println!(
        "Container image '{}' is available via {}.",
        runtime.tag,
        runtime.binary()
    );

    if let Some(dir) = &oci_dir {
        println!("  - Exported OCI directory: {}", dir.display());
    }
    if let Some(archive) = &oci_archive {
        println!("  - Exported OCI archive: {}", archive.display());
    }

    Ok(())
}

fn resolve_target_platform(
    manifest_ctx: &crate::app::ManifestContext,
    requested: Option<&str>,
) -> Result<Platform> {
    if let Some(raw) = requested {
        Platform::from_str(raw).map_err(|err| anyhow!(err))
    } else {
        let platforms = crate::conda::resolve_target_platforms(manifest_ctx.config.platforms())?;
        match platforms.as_slice() {
            [single] => Ok(*single),
            [] => bail!("no target platforms specified in manifest"),
            _ => bail!("manifest specifies multiple platforms; use --platform to select one"),
        }
    }
}

fn ensure_linux_platform(platform: Platform) -> Result<()> {
    if platform.as_str().starts_with("linux-") {
        Ok(())
    } else {
        bail!(
            "container builds are only supported for linux platforms (received '{}')",
            platform.as_str()
        )
    }
}

#[derive(Debug, Clone)]
struct RuntimeConfig {
    kind: RuntimeKind,
    tag: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeKind {
    Docker,
    Podman,
}

impl RuntimeConfig {
    fn binary(&self) -> &str {
        match self.kind {
            RuntimeKind::Docker => "docker",
            RuntimeKind::Podman => "podman",
        }
    }
}

fn select_runtime(docker_tag: Option<String>, podman_tag: Option<String>) -> Result<RuntimeConfig> {
    match (docker_tag, podman_tag) {
        (Some(docker_tag), None) => Ok(RuntimeConfig {
            kind: RuntimeKind::Docker,
            tag: docker_tag,
        }),
        (None, Some(podman_tag)) => Ok(RuntimeConfig {
            kind: RuntimeKind::Podman,
            tag: podman_tag,
        }),
        (Some(_), Some(_)) => bail!("please specify either --docker or --podman, not both"),
        (None, None) => bail!("container builds require either --docker <tag> or --podman <tag>"),
    }
}

fn validate_output_options(
    runtime: &RuntimeConfig,
    oci_dir: Option<&PathBuf>,
    oci_archive: Option<&PathBuf>,
) -> Result<()> {
    if runtime.kind == RuntimeKind::Docker {
        if oci_dir.is_some() || oci_archive.is_some() {
            bail!(
                "OCI outputs (--oci-dir/--oci-archive) require --podman because docker does not support exporting OCI layouts"
            );
        }
    }
    Ok(())
}

async fn prepare_self_extracting_installer(
    multi: &MultiProgress,
    step_style: &ProgressStyle,
    prep: &EnvironmentPreparation,
    platform: Platform,
) -> Result<PathBuf> {
    let installer_dir = prep.staging_dir.path().join("installers");
    fs::create_dir_all(&installer_dir).with_context(|| {
        format!(
            "failed to prepare installer staging directory {}",
            installer_dir.display()
        )
    })?;

    let progress = multi.add(ProgressBar::new_spinner());
    progress.set_style(step_style.clone());
    progress.set_prefix("[…]");
    progress.set_message("Prepare installer bundle");
    progress.enable_steady_tick(Duration::from_millis(120));

    let result = installer::create_installers(
        &installer_dir,
        &prep.environment_name,
        &prep.channel_dir,
        &[platform],
        &prep.bundle_metadata,
        &progress,
    );

    match result {
        Ok(paths) => {
            progress.set_prefix("[✔]");
            progress.finish_with_message("Prepare installer bundle (1/1)");
            paths
                .into_iter()
                .next()
                .ok_or_else(|| anyhow!("installer creation produced no outputs"))
        }
        Err(err) => {
            progress.set_prefix("[✖]");
            progress.finish_with_message("Prepare installer bundle (failed)");
            Err(err)
        }
    }
}

fn prepare_build_directory(manifest_dir: &Path, environment_name: &str) -> Result<PathBuf> {
    let base_dir = manifest_dir.join(".conda-dist").join("container");
    fs::create_dir_all(&base_dir).with_context(|| {
        format!(
            "failed to create container build root at {}",
            base_dir.display()
        )
    })?;

    let context_dir = base_dir.join(environment_name);
    if context_dir.exists() {
        fs::remove_dir_all(&context_dir).with_context(|| {
            format!(
                "failed to reset existing container build directory {}",
                context_dir.display()
            )
        })?;
    }
    fs::create_dir_all(&context_dir).with_context(|| {
        format!(
            "failed to prepare container build directory {}",
            context_dir.display()
        )
    })?;

    Ok(context_dir)
}

struct BuildContext {
    dir: PathBuf,
}

fn create_build_context(
    context_dir: &Path,
    installer_path: &Path,
    container_cfg: &ContainerConfig,
    install_prefix: &str,
    environment_name: &str,
) -> Result<BuildContext> {
    let dockerfile_path = context_dir.join("Dockerfile");
    let staged_installer = context_dir.join(INSTALLER_FILENAME);

    if staged_installer.exists() {
        fs::remove_file(&staged_installer).with_context(|| {
            format!(
                "failed to remove stale installer {}",
                staged_installer.display()
            )
        })?;
    }

    fs::copy(installer_path, &staged_installer).with_context(|| {
        format!(
            "failed to copy installer into build context ({})",
            staged_installer.display()
        )
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&staged_installer)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&staged_installer, perms)?;
    }

    let dockerfile_contents = format!(
        r#"FROM {builder} AS builder
COPY {installer} /tmp/{installer}
RUN /bin/bash -c "set -euo pipefail \
 && chmod +x /tmp/{installer} \
 && if ! /tmp/{installer} \"{prefix}\" > /tmp/conda-dist-install.log 2>&1; then \
      cat /tmp/conda-dist-install.log; \
      exit 1; \
    fi \
 && rm -f /tmp/{installer}"

FROM {base}
COPY --from=builder "{prefix}" "{prefix}"
ENV CONDA_PREFIX="{prefix}" \
    PATH="{prefix}/bin:${{PATH}}"
LABEL org.opencontainers.image.title="{title}"
"#,
        builder = container_cfg.builder_image,
        installer = INSTALLER_FILENAME,
        prefix = install_prefix,
        base = container_cfg.base_image,
        title = environment_name
    );

    fs::write(&dockerfile_path, dockerfile_contents).with_context(|| {
        format!(
            "failed to write Dockerfile to {}",
            dockerfile_path.display()
        )
    })?;

    if fs::metadata(&dockerfile_path)?.len() == 0 {
        bail!("generated Dockerfile was unexpectedly empty");
    }

    Ok(BuildContext {
        dir: context_dir.to_path_buf(),
    })
}

async fn build_image(
    runtime: &RuntimeConfig,
    context: &BuildContext,
    platform: Platform,
) -> Result<()> {
    let platform_spec = platform_to_runtime_spec(platform)?;
    let mut cmd = Command::new(runtime.binary());
    let context_path = &context.dir;
    let dockerfile_path = context_path.join("Dockerfile");
    cmd.arg("build")
        .arg("--platform")
        .arg(platform_spec)
        .arg("-t")
        .arg(&runtime.tag)
        .arg("--file")
        .arg(&dockerfile_path)
        .arg(context_path);

    run_command(&mut cmd, "image build").await
}

async fn export_optional_artifacts(
    runtime: &RuntimeConfig,
    oci_dir: Option<&PathBuf>,
    oci_archive: Option<&PathBuf>,
    tag: &str,
) -> Result<()> {
    if runtime.kind == RuntimeKind::Podman {
        if let Some(dir) = oci_dir {
            if dir.exists() {
                bail!(
                    "cannot export OCI directory to '{}' because it already exists",
                    dir.display()
                );
            }
            if let Some(parent) = dir.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("failed to prepare parent directory for {}", dir.display())
                })?;
            }

            let mut cmd = Command::new(runtime.binary());
            cmd.args(["image", "save", "--format", "oci-dir", "-o"])
                .arg(dir)
                .arg(tag);
            run_command(&mut cmd, "podman image save (oci-dir)").await?;
        }

        if let Some(archive) = oci_archive {
            if let Some(parent) = archive.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!(
                        "failed to prepare parent directory for {}",
                        archive.display()
                    )
                })?;
            }

            let mut cmd = Command::new(runtime.binary());
            cmd.args(["image", "save", "--format", "oci-archive", "-o"])
                .arg(archive)
                .arg(tag);
            run_command(&mut cmd, "podman image save (oci-archive)").await?;
        }
    }

    Ok(())
}

fn platform_to_runtime_spec(platform: Platform) -> Result<&'static str> {
    match platform {
        Platform::Linux64 => Ok("linux/amd64"),
        Platform::LinuxAarch64 => Ok("linux/arm64"),
        Platform::LinuxPpc64le => Ok("linux/ppc64le"),
        Platform::LinuxS390X => Ok("linux/s390x"),
        Platform::Linux32 => Ok("linux/386"),
        Platform::LinuxArmV7l => Ok("linux/arm/v7"),
        other => bail!(
            "unsupported platform '{}' for container builds",
            other.as_str()
        ),
    }
}

async fn run_command(cmd: &mut Command, action: &str) -> Result<()> {
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
            "{} failed (exit code {}):\n{}\n{}",
            action,
            output
                .status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "signal".to_string()),
            stdout,
            stderr
        )
    }
}
