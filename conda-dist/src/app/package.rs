//! Native RPM/DEB packaging entrypoints and shared orchestration helpers.
//!
//! High-level flow:
//! 1. Load and validate the manifest, resolve target platforms, and prepare the environment.
//! 2. Build installer bundles for the target platforms.
//! 3. Gather dependency file lists for modular builds.
//! 4. For each image/platform pair, write package_plan.tsv describing specs, roots, and payload lists.
//! 5. Run the containerized build script, which installs payloads and emits RPM/DEB artifacts.

use std::{
    collections::{BTreeSet, HashSet},
    env, fs,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Result, anyhow, bail};
use std::os::unix::fs::PermissionsExt;
use tempfile::Builder as TempDirBuilder;

use crate::{
    cli::PackageArgs,
    config::{PackageConfig, PackageFormat, PackageImageConfig},
    progress::Progress,
    workspace::Workspace,
};
mod bundle;
mod dependency_package_files;
mod model;
mod modular;
mod plan;
mod render;
mod runner;

use super::{
    LockMode, artifact_name,
    context::{ManifestContext, load_manifest_context},
    environment::{EnvironmentPreparation, prepare_environment},
    installer::prepare_installers,
    runtime::{self, RuntimeBinary},
};

const OUTPUT_DEST_PATH: &str = "/output";

use dependency_package_files::collect_dependencies;
use model::{PackageMode, PackageResult, install_prefix};
use plan::write_package_plan;
use runner::run_package;

/// Build native RPM/DEB packages from a conda-dist manifest using containerized tools.
pub async fn execute(
    args: PackageArgs,
    work_dir: Option<PathBuf>,
    engine: Option<PathBuf>,
    engine_flags: Vec<String>,
    lock_mode: LockMode,
) -> Result<()> {
    let PackageArgs {
        manifest,
        image,
        platform,
        output_dir,
    } = args;
    let manifest = load_manifest_context(manifest)?;
    let images = select_images(manifest.config.package(), image)?;
    if images.is_empty() {
        return Ok(());
    }
    let workspace = Workspace::from_manifest_dir(&manifest.manifest_dir, work_dir)?;
    let runtime = runtime::resolve_runtime(engine, engine_flags)?;
    let platforms = platform
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let progress = Progress::stdout();
    let prep = prepare_environment(&manifest, &workspace, platforms, lock_mode, &progress).await?;
    let prefix = install_prefix(&manifest, &prep)?;
    let output_root = match output_dir {
        Some(path) => env::current_dir()?.join(path),
        None => manifest.manifest_dir.clone(),
    };
    fs::create_dir_all(&output_root)?;
    let packaging_root = workspace.native_packaging_dir();
    fs::create_dir_all(&packaging_root)?;
    let packaging_dir = TempDirBuilder::new()
        .prefix(&format!("{}-", prep.environment_name))
        .tempdir_in(&packaging_root)?;
    let builds = plan_builds(
        &manifest,
        &prep,
        &workspace,
        &images,
        packaging_dir.path(),
        &output_root,
        &progress,
    )
    .await?;
    let results = build_packages(&runtime, &prefix, builds, &progress).await?;
    drop(progress);
    print_results(&results);
    Ok(())
}

fn select_images(
    config: &PackageConfig,
    requested: Vec<String>,
) -> Result<Vec<(PackageMode, String, PackageImageConfig)>> {
    let requested: HashSet<_> = requested.into_iter().collect();
    let mut images: Vec<_> = [
        (PackageMode::Bundle, &config.bundle),
        (PackageMode::Modular, &config.modular),
    ]
    .into_iter()
    .flat_map(|(mode, config)| {
        config
            .images
            .iter()
            .map(move |(name, image)| (mode, name.clone(), image.clone()))
    })
    .collect();
    if !requested.is_empty() {
        let missing: Vec<_> = requested
            .iter()
            .filter(|name| !images.iter().any(|(_, image_name, _)| image_name == *name))
            .cloned()
            .collect();
        if !missing.is_empty() {
            bail!(
                "requested package image(s) not defined in manifest: {}",
                missing.join(", ")
            );
        }
        images.retain(|(_, name, _)| requested.contains(name));
    }
    Ok(images)
}

async fn plan_builds(
    manifest: &ManifestContext,
    prep: &EnvironmentPreparation,
    workspace: &Workspace,
    images: &[(PackageMode, String, PackageImageConfig)],
    packaging_root: &Path,
    output_root: &Path,
    progress: &Progress,
) -> Result<Vec<plan::NativeBuild>> {
    let bundle_subsets: Vec<_> = std::iter::once(None)
        .chain(
            manifest
                .config
                .subsets()
                .keys()
                .map(|name| Some(name.as_str())),
        )
        .collect();
    let modular = images
        .iter()
        .any(|(mode, _, _)| *mode == PackageMode::Modular);
    let installers = prepare_installers(
        prep,
        &manifest.config,
        &packaging_root.join("installers"),
        progress,
    )
    .await?;
    let rpm_script = render::write_script(PackageFormat::Rpm, packaging_root)?;
    let deb_script = render::write_script(PackageFormat::Deb, packaging_root)?;
    let mut builds = Vec::new();
    for platform in &prep.target_platforms {
        let dependencies = if modular {
            collect_dependencies(prep, workspace, *platform).await?
        } else {
            Vec::new()
        };
        for (mode, name, image) in images {
            let subsets = match mode {
                PackageMode::Bundle => bundle_subsets.as_slice(),
                PackageMode::Modular => &[None],
            };
            let format = image.package_type;
            let script_path = match format {
                PackageFormat::Rpm => &rpm_script,
                PackageFormat::Deb => &deb_script,
            };
            let output_dir = output_root.join(mode.label()).join(name);
            fs::create_dir_all(&output_dir)?;
            for subset in subsets {
                let artifact = artifact_name(manifest.config.name(), *subset);
                let installer_path = installers
                    .get(&artifact)
                    .and_then(|paths| paths.get(platform))
                    .ok_or_else(|| {
                        anyhow!("no installer available for {artifact} on {platform}")
                    })?;
                let layout_name = match mode {
                    PackageMode::Bundle => {
                        format!("{name}/{}", artifact_name(manifest.config.name(), *subset))
                    }
                    PackageMode::Modular => name.clone(),
                };
                let plan_rel = write_package_plan(
                    format,
                    &layout_name,
                    *platform,
                    packaging_root,
                    plan::PackagePlanInput {
                        mode: *mode,
                        subset: *subset,
                        dependency_packages: &dependencies,
                    },
                    manifest,
                    prep,
                )?;
                builds.push(plan::NativeBuild {
                    format,
                    image_name: name.clone(),
                    image: image.clone(),
                    platform: *platform,
                    script_path: script_path.clone(),
                    output_dir: output_dir.clone(),
                    installer_path: installer_path.clone(),
                    packaging_root: packaging_root.to_path_buf(),
                    plan_rel,
                    subset: subset.map(str::to_owned),
                    mode: *mode,
                });
            }
        }
    }
    Ok(builds)
}

async fn build_packages(
    runtime: &RuntimeBinary,
    prefix: &str,
    builds: Vec<plan::NativeBuild>,
    progress: &Progress,
) -> Result<Vec<PackageResult>> {
    let count = builds.len();
    progress
        .step("Build native packages")
        .run_with(
            Some(Duration::from_millis(120)),
            |handle| async move {
                let mut counter = handle.counter(count);
                let mut produced = Vec::new();
                for (index, build) in builds.into_iter().enumerate() {
                    produced.extend(run_package(runtime, prefix, build).await?);
                    counter.set(index + 1);
                }
                Ok(produced)
            },
            |_| format!("Build native packages ({count}/{count})"),
        )
        .await
}

fn print_results(results: &[PackageResult]) {
    for (format, label) in [(PackageFormat::Rpm, "RPM"), (PackageFormat::Deb, "DEB")] {
        let count = results
            .iter()
            .filter(|result| result.format == format)
            .count();
        if count > 0 {
            println!("Generated {count} {label} package(s).");
        }
    }
    if !results.is_empty() {
        println!("Native package outputs:");
        for result in results {
            println!(
                "  - [{} {} {}] {}",
                result.format.label(),
                result.platform.as_str(),
                result.image_name,
                result.path.display(),
            );
        }
    }
}

/// Write a container packaging script and mark it executable for runtime use.
fn write_script(path: &Path, contents: &str) -> Result<()> {
    fs::write(path, contents)?;

    {
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms)?;
    }

    Ok(())
}
