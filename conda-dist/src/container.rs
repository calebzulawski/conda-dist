use std::{
    env, fs,
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use indicatif::ProgressBar;
use rattler_conda_types::Platform;
use tokio::process::Command;

use crate::{
    app::{EnvironmentPreparation, load_manifest_context, prepare_environment},
    cli::ContainerArgs,
    config::ContainerConfig,
    installer,
    progress::Progress,
    workspace::Workspace,
};

const INSTALLER_FILENAME: &str = "installer.sh";

pub async fn execute(args: ContainerArgs, work_dir: Option<PathBuf>) -> Result<()> {
    let ContainerArgs {
        manifest,
        platform,
        engine,
        unlock,
    } = args;

    let manifest_ctx = load_manifest_context(manifest)?;
    let container_cfg = manifest_ctx.config.container().cloned().unwrap_or_default();
    let workspace = Workspace::from_manifest_dir(&manifest_ctx.manifest_dir, work_dir)?;

    let target_platform = resolve_target_platform(&manifest_ctx, platform.as_deref())?;
    ensure_linux_platform(target_platform)?;

    let engine_path = resolve_runtime(engine)?;
    let image_tag = derive_image_tag(&manifest_ctx, &container_cfg)?;
    let runtime = RuntimeConfig::new(engine_path, image_tag);

    let progress = Progress::stdout();
    let mut final_messages = Vec::new();

    let (prep, _) = prepare_environment(
        &manifest_ctx,
        &workspace,
        vec![target_platform],
        unlock,
        &progress,
    )
    .await?;

    let installer_step = progress.step("Prepare installer bundle");
    let installer_bar = installer_step.clone_bar();
    let prep_ref = &prep;
    let installer_path =
        installer_step
            .run(
                Some(Duration::from_millis(120)),
                async move {
                    prepare_self_extracting_installer(&installer_bar, prep_ref, target_platform)
                },
                |_| "Prepare installer bundle (1/1)".to_string(),
            )
            .await?;

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

    let context_dir = prepare_build_directory(&workspace, &prep.environment_name)?;
    let build_context = create_build_context(
        &context_dir,
        &installer_path,
        &container_cfg,
        &install_prefix,
        &prep.environment_name,
    )?;

    let build_step = progress.step("Build container image");
    let runtime_ref = &runtime;
    let build_context_ref = &build_context;
    build_step
        .run(
            Some(Duration::from_millis(120)),
            async move { build_image(runtime_ref, build_context_ref, target_platform).await },
            |_| "Build container image (1/1)".to_string(),
        )
        .await?;

    final_messages.push(format!(
        "Container image '{}' is available via {}.",
        runtime.tag,
        runtime.binary().display()
    ));

    drop(progress);

    for message in final_messages {
        println!("{}", message);
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
    binary: PathBuf,
    tag: String,
}

impl RuntimeConfig {
    fn new(binary: PathBuf, tag: String) -> Self {
        Self { binary, tag }
    }

    fn binary(&self) -> &Path {
        &self.binary
    }
}

fn resolve_runtime(engine: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(explicit) = engine {
        if !is_supported_engine(&explicit) {
            bail!(
                "unable to determine engine type for {} (expected docker or podman binary)",
                explicit.display()
            );
        }
        return Ok(explicit);
    }

    if let Some(path) = find_in_path("docker") {
        return Ok(path);
    }

    if let Some(path) = find_in_path("podman") {
        return Ok(path);
    }

    bail!("no container engine found; install docker or podman, or supply --engine <path>");
}

fn is_supported_engine(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    let lowercase = name.to_lowercase();
    lowercase.contains("docker") || lowercase.contains("podman")
}

fn find_in_path(binary: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    env::split_paths(&path_var)
        .map(|dir| dir.join(binary))
        .find(|candidate| candidate.is_file())
}

fn derive_image_tag(
    manifest_ctx: &crate::app::ManifestContext,
    container_cfg: &ContainerConfig,
) -> Result<String> {
    let name = manifest_ctx.config.name();
    let version = manifest_ctx.config.version().ok_or_else(|| {
        anyhow!("manifest is missing required field 'version' for container builds")
    })?;
    let version = version.trim();
    if version.is_empty() {
        bail!("manifest 'version' field cannot be empty for container builds");
    }
    if version.chars().any(|ch| ch.is_whitespace()) {
        bail!("manifest 'version' field must not contain whitespace");
    }

    let mut segments = Vec::new();
    if let Some(registry) = container_cfg
        .registry
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        segments.push(registry.trim_matches('/').to_string());
    }
    if let Some(org) = container_cfg
        .organization
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        segments.push(org.trim_matches('/').to_string());
    }
    segments.push(name.to_string());

    let repository = segments.join("/");
    Ok(format!("{repository}:{version}"))
}

fn prepare_self_extracting_installer(
    progress: &ProgressBar,
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

    let result = installer::create_installers(
        &installer_dir,
        &prep.environment_name,
        &prep.channel_dir,
        &[platform],
        &prep.bundle_metadata,
        progress,
    );
    let paths = result?;
    paths
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("installer creation produced no outputs"))
}

fn prepare_build_directory(workspace: &Workspace, environment_name: &str) -> Result<PathBuf> {
    let base_dir = workspace.container_root();
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
