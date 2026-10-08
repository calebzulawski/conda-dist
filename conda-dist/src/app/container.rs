use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use askama::Template;
use rattler_conda_types::Platform;

use crate::{
    cli::ContainerArgs, config::ContainerConfig, progress::Progress, workspace::Workspace,
};

use super::{
    LockMode, artifact_name,
    context::{ManifestContext, load_manifest_context},
    environment::prepare_environment,
    installer::prepare_installers,
    runtime::{self, RuntimeBinary, RuntimeEngine},
};

pub async fn execute(
    args: ContainerArgs,
    work_dir: Option<PathBuf>,
    engine: Option<PathBuf>,
    engine_flags: Vec<String>,
    lock_mode: LockMode,
) -> Result<()> {
    let ContainerArgs { manifest, platform } = args;
    let manifest_ctx = load_manifest_context(manifest)?;
    let config = &manifest_ctx.config;
    let container_cfg = config.container().cloned().unwrap_or_default();
    let workspace = Workspace::from_manifest_dir(&manifest_ctx.manifest_dir, work_dir)?;
    let target_platforms = resolve_target_platforms(&manifest_ctx, platform)?;
    let runtime_binary = runtime::resolve_runtime(engine, engine_flags)?;
    let artifacts = plan_images(&manifest_ctx, &container_cfg)?;
    let progress = Progress::stdout();
    let prep = prepare_environment(
        &manifest_ctx,
        &workspace,
        target_platforms.clone(),
        lock_mode,
        &progress,
    )
    .await?;
    let install_prefix = container_cfg
        .prefix
        .clone()
        .unwrap_or_else(|| format!("/opt/{}", prep.environment_name));
    if !install_prefix.starts_with('/') {
        bail!("container prefix '{install_prefix}' must be an absolute path");
    }
    let installers_root = prep.staging_dir.path().join("installers");
    let installers = prepare_installers(&prep, config, &installers_root, &progress).await?;
    let mut messages = Vec::new();
    for ImageArtifact {
        subset,
        name,
        tag,
        archive,
    } in artifacts
    {
        let runtime = RuntimeConfig {
            binary: runtime_binary.clone(),
            tag,
        };
        let context_dir = prepare_build_directory(&workspace, &name)?;
        let context = create_build_context(
            &context_dir,
            installers
                .get(&name)
                .with_context(|| format!("missing installer for {name}"))?,
            &container_cfg,
            &install_prefix,
            &name,
            archive,
            subset,
        )?;
        let archive = progress
            .step(format!("Build container image {name}"))
            .run(
                Some(Duration::from_millis(120)),
                build_image(&runtime, &context, &target_platforms),
                |_| format!("Build container image {name} (1/1)"),
            )
            .await?;
        messages.push(format!(
            "Container image '{}' prepared for {} linux platform(s): {}.",
            runtime.tag,
            target_platforms.len(),
            runtime::format_platform_list(&target_platforms)
        ));
        messages.push(format!(
            "Multi-platform OCI archive written to {}",
            archive.display()
        ));
    }
    drop(progress);
    for message in messages {
        println!("{message}");
    }
    Ok(())
}

struct ImageArtifact<'a> {
    subset: Option<&'a str>,
    name: String,
    tag: String,
    archive: PathBuf,
}

fn plan_images<'a>(
    manifest: &'a ManifestContext,
    container: &ContainerConfig,
) -> Result<Vec<ImageArtifact<'a>>> {
    let config = &manifest.config;
    let mut tags = std::collections::HashSet::new();
    let mut artifacts = Vec::new();
    for subset in
        std::iter::once(None).chain(config.subsets().keys().map(|name| Some(name.as_str())))
    {
        let name = artifact_name(config.name(), subset);
        let tag = derive_image_tag(&name, config.version(), container)?;
        if !tags.insert(tag.clone()) {
            bail!(
                "container tag template produces duplicate tag '{tag}'; include {{name}} in the template"
            );
        }
        let archive = manifest
            .manifest_dir
            .join(format!("{name}-container.oci.tar"));
        artifacts.push(ImageArtifact {
            subset,
            name,
            tag,
            archive,
        });
    }
    Ok(artifacts)
}

fn resolve_target_platforms(
    manifest_ctx: &ManifestContext,
    requested: Option<Platform>,
) -> Result<Vec<Platform>> {
    if let Some(platform) = requested {
        if !is_linux_platform(platform) {
            bail!(
                "container builds are only supported for linux platforms (received '{}')",
                platform.as_str()
            );
        }
        if !manifest_ctx.config.platforms().contains(&platform) {
            bail!("selected platform '{platform}' is not listed in the manifest platforms");
        }
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

fn is_linux_platform(platform: Platform) -> bool {
    platform.as_str().starts_with("linux-")
}

#[derive(Debug, Clone)]
struct RuntimeConfig {
    binary: RuntimeBinary,
    tag: String,
}

fn derive_image_tag(name: &str, version: &str, container_cfg: &ContainerConfig) -> Result<String> {
    let version = version.trim();

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

#[derive(Template)]
#[template(path = "container.Dockerfile.txt", escape = "none")]
struct ContainerDockerfileTemplate<'a> {
    base_image: &'a str,
    prefix: &'a str,
    title: &'a str,
    installer_command: String,
}

fn create_build_context(
    context_dir: &Path,
    installers: &BTreeMap<Platform, PathBuf>,
    container_cfg: &ContainerConfig,
    install_prefix: &str,
    environment_name: &str,
    oci_archive: PathBuf,
    subset: Option<&str>,
) -> Result<BuildContext> {
    let dockerfile_path = context_dir.join("Dockerfile");
    let installers_dir = context_dir.join("installers");
    stage_installers(&installers_dir, installers)?;

    let dockerfile_contents = ContainerDockerfileTemplate {
        base_image: &container_cfg.base_image,
        prefix: install_prefix,
        title: environment_name,
        installer_command: serde_json::to_string(&match subset {
            Some(_) => vec!["/tmp/installer", install_prefix],
            None => vec!["/tmp/installer", install_prefix, "--components", "none"],
        })?,
    }
    .render()?;

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
        oci_archive,
    })
}

fn stage_installers(installers_dir: &Path, installers: &BTreeMap<Platform, PathBuf>) -> Result<()> {
    fs::create_dir_all(installers_dir).with_context(|| {
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

    Ok(())
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
    let mut cmd = runtime.binary.command();
    cmd.arg("buildx").arg("build");
    cmd.args(runtime.binary.engine_flags());
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

    let mut cmd = runtime.binary.command();
    cmd.arg("build");
    cmd.args(runtime.binary.engine_flags());
    cmd.arg("--platform")
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
    let archive_spec = format!("oci-archive:{}", output_path.to_string_lossy());

    let mut cmd = runtime.binary.command();
    cmd.arg("manifest")
        .arg("push")
        .arg("--all")
        .arg(&runtime.tag)
        .arg(&archive_spec);

    runtime::run_command(&mut cmd, "podman manifest push").await
}

async fn podman_manifest_remove(runtime: &RuntimeConfig) -> Result<()> {
    let mut cmd = runtime.binary.command();
    cmd.arg("manifest").arg("rm").arg(&runtime.tag);

    runtime::run_command(&mut cmd, "podman manifest rm")
        .await
        .ok();
    Ok(())
}
