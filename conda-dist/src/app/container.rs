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
    cli::ContainerArgs, config::ContainerConfig, installer, progress::Progress,
    workspace::Workspace,
};

use super::{
    context::{ManifestContext, load_manifest_context},
    environment::{EnvironmentPreparation, prepare_environment},
};

pub async fn execute(args: ContainerArgs, work_dir: Option<PathBuf>) -> Result<()> {
    let ContainerArgs {
        manifest,
        platform,
        engine,
        unlock,
        oci_output,
    } = args;

    let manifest_ctx = load_manifest_context(manifest)?;
    let container_cfg = manifest_ctx.config.container().cloned().unwrap_or_default();
    let workspace = Workspace::from_manifest_dir(&manifest_ctx.manifest_dir, work_dir)?;

    let target_platforms = resolve_target_platforms(&manifest_ctx, platform.as_deref())?;
    ensure_linux_platforms(&target_platforms)?;

    let (engine_path, engine) = resolve_runtime(engine)?;
    let image_tag = derive_image_tag(&manifest_ctx, &container_cfg)?;
    let runtime = RuntimeConfig::new(engine_path, engine, image_tag);

    let progress = Progress::stdout();
    let mut final_messages = Vec::new();

    let (prep, _) = prepare_environment(
        &manifest_ctx,
        &workspace,
        target_platforms.clone(),
        unlock,
        &progress,
    )
    .await?;

    let oci_output_path = match oci_output {
        Some(path) => {
            if path.is_absolute() {
                path
            } else {
                env::current_dir()?.join(path)
            }
        }
        None => manifest_ctx
            .manifest_dir
            .join(format!("{}-container.oci.tar", prep.environment_name)),
    };

    let platform_summary = format_platform_list(&target_platforms);

    let installer_label = format!("Prepare installer bundle [{}]", platform_summary);
    let installer_step = progress.step(installer_label.clone());
    let installer_bar = installer_step.clone_bar();
    let prep_ref = &prep;
    let installer_platforms = target_platforms.clone();
    let installers = installer_step
        .run(
            Some(Duration::from_millis(120)),
            async move {
                prepare_self_extracting_installers(&installer_bar, prep_ref, &installer_platforms)
            },
            move |_| installer_label.clone(),
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
        &installers,
        &container_cfg,
        &install_prefix,
        &prep.environment_name,
        oci_output_path.clone(),
    )?;

    let build_step = progress.step("Build container image");
    let runtime_ref = &runtime;
    let build_context_ref = &build_context;
    let build_platforms = target_platforms.clone();
    let platform_count = build_platforms.len();
    let archive_path = build_step
        .run(
            Some(Duration::from_millis(120)),
            async move { build_image(runtime_ref, build_context_ref, &build_platforms).await },
            |_| "Build container image (1/1)".to_string(),
        )
        .await?;

    final_messages.push(format!(
        "Container image '{}' prepared for {} linux platform(s): {}.",
        runtime.tag, platform_count, platform_summary
    ));
    final_messages.push(format!(
        "Multi-platform OCI archive written to {}",
        archive_path.display()
    ));

    drop(progress);

    for message in final_messages {
        println!("{}", message);
    }

    Ok(())
}

fn resolve_target_platforms(
    manifest_ctx: &ManifestContext,
    requested: Option<&str>,
) -> Result<Vec<Platform>> {
    if let Some(raw) = requested {
        let platform = Platform::from_str(raw.trim()).map_err(|err| anyhow!(err))?;
        Ok(vec![platform])
    } else {
        let platforms = crate::conda::resolve_target_platforms(manifest_ctx.config.platforms())?;
        let linux_platforms: Vec<Platform> = platforms
            .into_iter()
            .filter(|platform| is_linux_platform(*platform))
            .collect();

        if linux_platforms.is_empty() {
            bail!("no linux target platforms specified in manifest");
        }

        Ok(linux_platforms)
    }
}

fn ensure_linux_platforms(platforms: &[Platform]) -> Result<()> {
    if let Some(non_linux) = platforms
        .iter()
        .find(|platform| !is_linux_platform(**platform))
    {
        bail!(
            "container builds are only supported for linux platforms (received '{}')",
            non_linux.as_str()
        );
    }

    Ok(())
}

fn is_linux_platform(platform: Platform) -> bool {
    platform.as_str().starts_with("linux-")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeEngine {
    Docker,
    Podman,
}

#[derive(Debug, Clone)]
struct RuntimeConfig {
    binary: PathBuf,
    engine: RuntimeEngine,
    tag: String,
}

impl RuntimeConfig {
    fn new(binary: PathBuf, engine: RuntimeEngine, tag: String) -> Self {
        Self {
            binary,
            engine,
            tag,
        }
    }

    fn binary(&self) -> &Path {
        &self.binary
    }

    fn engine(&self) -> RuntimeEngine {
        self.engine
    }
}

fn resolve_runtime(engine: Option<PathBuf>) -> Result<(PathBuf, RuntimeEngine)> {
    if let Some(explicit) = engine {
        match detect_runtime_engine(&explicit) {
            Some(kind) => return Ok((explicit, kind)),
            None => bail!(
                "unable to determine engine type for {} (expected docker or podman binary)",
                explicit.display()
            ),
        }
    }

    if let Some(path) = find_in_path("docker") {
        return Ok((path, RuntimeEngine::Docker));
    }

    if let Some(path) = find_in_path("podman") {
        return Ok((path, RuntimeEngine::Podman));
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

fn derive_image_tag(
    manifest_ctx: &ManifestContext,
    container_cfg: &ContainerConfig,
) -> Result<String> {
    let name = manifest_ctx.config.name();
    let version = manifest_ctx.config.version().trim();
    if version.is_empty() {
        bail!("manifest 'version' field cannot be empty for container builds");
    }
    if version.chars().any(|ch| ch.is_whitespace()) {
        bail!("manifest 'version' field must not contain whitespace");
    }

    let template = container_cfg.tag_template.trim();
    if template.is_empty() {
        bail!("container 'tag_template' must not be empty");
    }

    let rendered = template
        .replace("{name}", name)
        .replace("{version}", version);
    if rendered.contains('{') || rendered.contains('}') {
        bail!(
            "container tag template contains unsupported placeholders; allowed variables: {{name}}, {{version}}"
        );
    }

    let tag = rendered.trim();
    if tag.is_empty() {
        bail!("container tag template resolved to an empty tag");
    }
    if tag.chars().any(|ch| ch.is_whitespace()) {
        bail!("container tag '{tag}' must not contain whitespace");
    }

    Ok(tag.to_string())
}

fn prepare_self_extracting_installers(
    progress: &ProgressBar,
    prep: &EnvironmentPreparation,
    platforms: &[Platform],
) -> Result<Vec<(Platform, PathBuf)>> {
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
        platforms,
        &prep.bundle_metadata,
        progress,
    );
    let paths = result?;

    if paths.len() != platforms.len() {
        bail!(
            "unexpected installer output; expected {} artifacts but received {}",
            platforms.len(),
            paths.len()
        );
    }

    Ok(platforms.iter().copied().zip(paths.into_iter()).collect())
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
    oci_archive: PathBuf,
}

fn create_build_context(
    context_dir: &Path,
    installers: &[(Platform, PathBuf)],
    container_cfg: &ContainerConfig,
    install_prefix: &str,
    environment_name: &str,
    oci_archive: PathBuf,
) -> Result<BuildContext> {
    if installers.is_empty() {
        bail!("no installers available to build container image");
    }

    let dockerfile_path = context_dir.join("Dockerfile");
    let installers_dir = context_dir.join("installers");

    if installers_dir.exists() {
        fs::remove_dir_all(&installers_dir).with_context(|| {
            format!(
                "failed to remove stale installers directory {}",
                installers_dir.display()
            )
        })?;
    }

    fs::create_dir_all(&installers_dir).with_context(|| {
        format!(
            "failed to prepare installers directory {}",
            installers_dir.display()
        )
    })?;

    for (platform, source_path) in installers {
        let spec = platform_to_runtime_spec(*platform)?;
        let arch = spec
            .split('/')
            .nth(1)
            .ok_or_else(|| anyhow!("unsupported runtime specification '{}'", spec))?;
        let filename = format!("installer-{}", arch);
        let staged_installer = installers_dir.join(&filename);

        if staged_installer.exists() {
            fs::remove_file(&staged_installer).with_context(|| {
                format!(
                    "failed to remove stale installer {}",
                    staged_installer.display()
                )
            })?;
        }

        fs::copy(source_path, &staged_installer).with_context(|| {
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
    }

    let dockerfile_contents = format!(
        r#"# syntax=docker/dockerfile:1.6
FROM scratch AS installer_payload
COPY installers/ /installers/

FROM {base}
ARG TARGETARCH
RUN --mount=type=bind,from=installer_payload,source=/installers/installer-${{TARGETARCH}},target=/tmp/installer,ro ["/tmp/installer", "{prefix}"]
ENV CONDA_PREFIX="{prefix}" \
    PATH="{prefix}/bin:${{PATH}}"
LABEL org.opencontainers.image.title="{title}"
"#,
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

    if let Some(parent) = oci_archive.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to prepare container artifact directory {}",
                parent.display()
            )
        })?;
    }

    if oci_archive.exists() {
        fs::remove_file(&oci_archive).with_context(|| {
            format!(
                "failed to remove existing OCI archive {}",
                oci_archive.display()
            )
        })?;
    }

    Ok(BuildContext {
        dir: context_dir.to_path_buf(),
        oci_archive,
    })
}

async fn build_image(
    runtime: &RuntimeConfig,
    context: &BuildContext,
    platforms: &[Platform],
) -> Result<PathBuf> {
    if platforms.is_empty() {
        bail!("no target platforms provided for container build");
    }

    let specs = platforms
        .iter()
        .map(|platform| platform_to_runtime_spec(*platform).map(|spec| spec.to_string()))
        .collect::<Result<Vec<_>>>()?;

    let context_path = &context.dir;
    let dockerfile_path = context_path.join("Dockerfile");
    let oci_archive = context.oci_archive.clone();

    if let Some(parent) = oci_archive.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to prepare OCI archive parent directory {}",
                parent.display()
            )
        })?;
    }
    if oci_archive.exists() {
        fs::remove_file(&oci_archive).with_context(|| {
            format!(
                "failed to remove existing OCI archive {}",
                oci_archive.display()
            )
        })?;
    }

    match runtime.engine() {
        RuntimeEngine::Docker => {
            build_with_docker(
                runtime,
                &dockerfile_path,
                context_path,
                &specs,
                &oci_archive,
            )
            .await?
        }
        RuntimeEngine::Podman => {
            build_with_podman(
                runtime,
                &dockerfile_path,
                context_path,
                &specs,
                &oci_archive,
            )
            .await?
        }
    }

    if !oci_archive.exists() {
        bail!(
            "container build completed but OCI archive was not created at {}",
            oci_archive.display()
        );
    }

    Ok(oci_archive)
}

async fn build_with_docker(
    runtime: &RuntimeConfig,
    dockerfile_path: &Path,
    context_path: &Path,
    specs: &[String],
    output_path: &Path,
) -> Result<()> {
    let mut cmd = Command::new(runtime.binary());
    cmd.arg("buildx").arg("build");
    let combined = specs.join(",");
    cmd.arg("--platform").arg(combined);
    cmd.arg("--tag").arg(&runtime.tag);
    cmd.arg("--file").arg(dockerfile_path).arg(context_path);

    let output_spec = format!("type=oci,dest={}", output_path.to_string_lossy());
    cmd.arg("--output").arg(output_spec);

    run_command(&mut cmd, "image build").await
}

async fn build_with_podman(
    runtime: &RuntimeConfig,
    dockerfile_path: &Path,
    context_path: &Path,
    specs: &[String],
    output_path: &Path,
) -> Result<()> {
    if specs.is_empty() {
        bail!("no platforms specified for podman build");
    }

    podman_manifest_remove(runtime).await.ok();

    let mut cmd = Command::new(runtime.binary());
    cmd.arg("build")
        .arg("--platform")
        .arg(specs.join(","))
        .arg("--manifest")
        .arg(&runtime.tag)
        .arg("--file")
        .arg(dockerfile_path)
        .arg(context_path);

    run_command(&mut cmd, "podman build").await?;
    podman_save_image(runtime, output_path).await
}

async fn podman_save_image(runtime: &RuntimeConfig, output_path: &Path) -> Result<()> {
    let archive_path = if output_path.is_absolute() {
        output_path.to_path_buf()
    } else {
        env::current_dir()
            .context("failed to resolve working directory for podman manifest push")?
            .join(output_path)
    };

    if let Some(parent) = archive_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to prepare directory {} for podman export",
                    parent.display()
                )
            })?;
        }
    }

    let archive_spec = format!("oci-archive:{}", archive_path.to_string_lossy());

    let mut cmd = Command::new(runtime.binary());
    cmd.arg("manifest")
        .arg("push")
        .arg("--all")
        .arg(&runtime.tag)
        .arg(&archive_spec);

    run_command(&mut cmd, "podman manifest push").await
}

async fn podman_manifest_remove(runtime: &RuntimeConfig) -> Result<()> {
    let mut cmd = Command::new(runtime.binary());
    cmd.arg("manifest").arg("rm").arg(&runtime.tag);

    run_command(&mut cmd, "podman manifest rm").await.ok();
    Ok(())
}

fn format_platform_list(platforms: &[Platform]) -> String {
    let mut names: Vec<&str> = platforms.iter().map(|p| p.as_str()).collect();
    names.sort_unstable();
    names.join(", ")
}

fn platform_to_runtime_spec(platform: Platform) -> Result<&'static str> {
    match platform {
        Platform::Linux64 => Ok("linux/amd64"),
        Platform::LinuxAarch64 => Ok("linux/arm64/v8"),
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
