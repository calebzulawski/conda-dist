use std::{
    env, fs,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use rattler_conda_types::Platform;
use tokio::process::Command;

use crate::{
    cli::ContainerArgs,
    config::ContainerConfig,
    installer,
    progress::{Progress, ProgressCounter},
    workspace::Workspace,
};

use super::{
    LockMode,
    context::{ManifestContext, load_manifest_context},
    environment::{EnvironmentPreparation, prepare_environment},
    runtime::{self, RuntimeBinary, RuntimeEngine},
};

pub async fn execute(
    args: ContainerArgs,
    work_dir: Option<PathBuf>,
    lock_mode: LockMode,
) -> Result<()> {
    let ContainerArgs {
        manifest,
        platform,
        engine,
        oci_output,
    } = args;

    let manifest_ctx = load_manifest_context(manifest)?;
    let container_cfg = manifest_ctx.config.container().cloned().unwrap_or_default();
    let workspace = Workspace::from_manifest_dir(&manifest_ctx.manifest_dir, work_dir)?;

    let target_platforms = resolve_target_platforms(&manifest_ctx, platform)?;
    ensure_linux_platforms(&target_platforms)?;

    let runtime_binary = runtime::resolve_runtime(engine)?;
    let image_tag = derive_image_tag(&manifest_ctx, &container_cfg)?;
    let runtime = RuntimeConfig {
        binary: runtime_binary,
        tag: image_tag,
    };

    let progress = Progress::stdout();
    let mut final_messages = Vec::new();

    let (prep, _, _) = prepare_environment(
        &manifest_ctx,
        &workspace,
        target_platforms.clone(),
        lock_mode,
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

    let platform_summary = runtime::format_platform_list(&target_platforms);

    let installer_label = format!("Prepare installer bundle [{platform_summary}]");
    let installer_step = progress.step(installer_label.clone());
    let prep_ref = &prep;
    let installer_platforms = target_platforms.clone();
    let total_installers = installer_platforms.len();
    let installers = installer_step
        .run_with(
            Some(Duration::from_millis(120)),
            {
                move |handle| async move {
                    let mut counter = handle.counter(total_installers);
                    prepare_self_extracting_installers(&mut counter, prep_ref, &installer_platforms)
                }
            },
            move |_| installer_label.clone(),
        )
        .await?;

    let install_prefix = container_cfg
        .prefix
        .clone()
        .unwrap_or_else(|| format!("/opt/{}", prep.environment_name));
    if !install_prefix.starts_with('/') {
        bail!("container prefix '{install_prefix}' must be an absolute path");
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
        println!("{message}");
    }

    Ok(())
}

fn resolve_target_platforms(
    manifest_ctx: &ManifestContext,
    requested: Option<Platform>,
) -> Result<Vec<Platform>> {
    if let Some(platform) = requested {
        Ok(vec![platform])
    } else {
        let platforms = manifest_ctx.config.platforms().to_vec();
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

#[derive(Debug, Clone)]
struct RuntimeConfig {
    binary: RuntimeBinary,
    tag: String,
}

fn derive_image_tag(
    manifest_ctx: &ManifestContext,
    container_cfg: &ContainerConfig,
) -> Result<String> {
    let name = manifest_ctx.config.name();
    let version = manifest_ctx.config.version().trim();

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
    progress: &mut ProgressCounter,
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

    Ok(platforms.iter().copied().zip(paths).collect())
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
        let spec = runtime::platform_to_runtime_spec(*platform)?;
        let arch = spec
            .split('/')
            .nth(1)
            .ok_or_else(|| anyhow!("unsupported runtime specification '{spec}'"))?;
        let filename = format!("installer-{arch}");
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
    let specs = platforms
        .iter()
        .map(|platform| runtime::platform_to_runtime_spec(*platform).map(|spec| spec.to_string()))
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

    match runtime.binary.engine() {
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
    let mut cmd = Command::new(runtime.binary.binary());
    cmd.arg("buildx").arg("build");
    let combined = specs.join(",");
    cmd.arg("--platform").arg(combined);
    cmd.arg("--tag").arg(&runtime.tag);
    cmd.arg("--file").arg(dockerfile_path).arg(context_path);

    let output_spec = format!("type=oci,dest={}", output_path.to_string_lossy());
    cmd.arg("--output").arg(output_spec);

    runtime::run_command(&mut cmd, "image build").await
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

    let mut cmd = Command::new(runtime.binary.binary());
    cmd.arg("build")
        .arg("--platform")
        .arg(specs.join(","))
        .arg("--manifest")
        .arg(&runtime.tag)
        .arg("--file")
        .arg(dockerfile_path)
        .arg(context_path);

    runtime::run_command(&mut cmd, "podman build").await?;
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

    if let Some(parent) = archive_path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to prepare directory {} for podman export",
                parent.display()
            )
        })?;
    }

    let archive_spec = format!("oci-archive:{}", archive_path.to_string_lossy());

    let mut cmd = Command::new(runtime.binary.binary());
    cmd.arg("manifest")
        .arg("push")
        .arg("--all")
        .arg(&runtime.tag)
        .arg(&archive_spec);

    runtime::run_command(&mut cmd, "podman manifest push").await
}

async fn podman_manifest_remove(runtime: &RuntimeConfig) -> Result<()> {
    let mut cmd = Command::new(runtime.binary.binary());
    cmd.arg("manifest").arg("rm").arg(&runtime.tag);

    runtime::run_command(&mut cmd, "podman manifest rm")
        .await
        .ok();
    Ok(())
}
