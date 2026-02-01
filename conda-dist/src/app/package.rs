//! Native RPM/DEB packaging entrypoints and shared orchestration helpers.
//!
//! High-level flow:
//! 1. Load and validate the manifest, resolve target platforms, and prepare the environment.
//! 2. Build installer bundles for the target platforms.
//! 3. Gather split dependency file lists (if enabled) and compose dependency package inputs.
//! 4. For each image/platform pair, write package_plan.tsv describing specs, roots, and payload lists.
//! 5. Run the containerized build script, which installs payloads and emits RPM/DEB artifacts.

use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Result, anyhow, bail};
use rattler_conda_types::Platform;
use std::os::unix::fs::PermissionsExt;
use tempfile::Builder as TempDirBuilder;

use crate::{
    cli::PackageArgs, config::PackageFormat, installer, progress::Progress, workspace::Workspace,
};
mod dependency_package_files;
mod model;
mod plan;
mod render;
mod runner;

use super::{
    LockMode,
    context::load_manifest_context,
    environment::prepare_environment,
    push_download_summary,
    runtime::{self},
};

const OUTPUT_DEST_PATH: &str = "/output";

use dependency_package_files::collect_dependencies;
use model::{DependencyPackage, ensure_linux_package_platform, install_prefix};
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

    let manifest_ctx = load_manifest_context(manifest)?;
    let workspace = Workspace::from_manifest_dir(&manifest_ctx.manifest_dir, work_dir)?;
    let runtime = runtime::resolve_runtime(engine, engine_flags)?;

    let requested_images: std::collections::HashSet<String> = image.into_iter().collect();
    let mut images_map = manifest_ctx.config.package().images.clone();
    if !requested_images.is_empty() {
        let missing: Vec<_> = requested_images
            .iter()
            .filter(|name| !images_map.contains_key(*name))
            .cloned()
            .collect();
        if !missing.is_empty() {
            bail!(
                "requested package image(s) not defined in manifest: {}",
                missing.join(", ")
            );
        }
        images_map.retain(|name, _| requested_images.contains(name));
    }
    if images_map.is_empty() {
        bail!("manifest must define at least one package image under [package.images]");
    }

    let requested_platforms = normalize_platforms(if platform.is_empty() {
        vec![Platform::current()]
    } else {
        platform
    });

    for platform in &requested_platforms {
        ensure_linux_package_platform(*platform)?;
    }

    let manifest_platforms = manifest_ctx.config.platforms().to_vec();
    for platform in &requested_platforms {
        if !manifest_platforms.contains(platform) {
            bail!(
                "selected platform '{}' is not listed in the manifest platforms",
                platform.as_str()
            );
        }
    }

    let progress = Progress::stdout();
    let mut final_messages = Vec::new();

    let (prep, download_summary, _) = prepare_environment(
        &manifest_ctx,
        &workspace,
        requested_platforms.clone(),
        lock_mode,
        &progress,
    )
    .await?;

    let split_deps = manifest_ctx.config.package().split_deps;

    let prefix = install_prefix(&manifest_ctx, &prep)?;
    let output_root = match output_dir {
        Some(path) => env::current_dir()?.join(path),
        None => manifest_ctx.manifest_dir.clone(),
    };
    fs::create_dir_all(&output_root)?;

    let packaging_root = workspace.native_packaging_dir();
    fs::create_dir_all(&packaging_root)?;
    let _packaging_tempdir = TempDirBuilder::new()
        .prefix(&format!("{}-", prep.environment_name))
        .tempdir_in(&packaging_root)?;
    let packaging_dir = _packaging_tempdir.path();

    let installer_summary = runtime::format_platform_list(&prep.target_platforms);
    let installer_label = format!("Prepare installer bundle [{installer_summary}]");
    let installer_step = progress.step(installer_label.clone());
    let prep_ref = &prep;
    let installer_platforms = prep.target_platforms.clone();
    let installer_root = packaging_dir.join("installers");
    let installers = installer_step
        .run_with(
            Some(Duration::from_millis(120)),
            move |handle| async move {
                let mut counter = handle.counter(installer_platforms.len());
                let paths = installer::create_installers(
                    &installer_root,
                    &prep_ref.environment_name,
                    &prep_ref.channel_dir,
                    &installer_platforms,
                    &prep_ref.bundle_metadata,
                    &mut counter,
                )?;
                Ok(installer_platforms.iter().copied().zip(paths).collect())
            },
            move |_: &Vec<(Platform, PathBuf)>| installer_label.clone(),
        )
        .await?;
    let installer_map: HashMap<Platform, PathBuf> = installers.into_iter().collect();

    let rpm_script = render::write_script(PackageFormat::Rpm, packaging_dir)?;
    let deb_script = render::write_script(PackageFormat::Deb, packaging_dir)?;

    struct PlatformBatch {
        platform: Platform,
        dependency_packages: Vec<DependencyPackage>,
    }

    let mut platform_batches = Vec::new();
    for platform in &prep.target_platforms {
        let dependency_packages = if split_deps {
            collect_dependencies(&manifest_ctx, &prep, &workspace, *platform).await?
        } else {
            Vec::new()
        };

        platform_batches.push(PlatformBatch {
            platform: *platform,
            dependency_packages,
        });
    }

    let mut package_count = 0;
    for batch in &platform_batches {
        let per_image = 1 + batch.dependency_packages.len();
        package_count += images_map.len() * per_image;
    }

    if package_count == 0 {
        bail!("no native package jobs were scheduled");
    }

    let packaging_step = progress.step("Build native packages");
    let runtime_clone = runtime.clone();
    let prefix_clone = prefix.clone();

    let results = packaging_step
        .run_with(
            Some(Duration::from_millis(120)),
            move |handle| async move {
                let mut counter = handle.counter(package_count);
                let mut produced = Vec::new();
                let mut completed = 0;

                for batch in platform_batches.into_iter() {
                    for (name, cfg) in images_map.iter() {
                        let format = cfg.package_type;
                        let script_path = match format {
                            PackageFormat::Rpm => rpm_script.as_path(),
                            PackageFormat::Deb => deb_script.as_path(),
                        };
                        let output_dir = output_root.join(name);
                        std::fs::create_dir_all(&output_dir)?;
                        let plan_rel = write_package_plan(
                            format,
                            name,
                            batch.platform,
                            packaging_dir,
                            &batch.dependency_packages,
                            &manifest_ctx,
                            &prep,
                        )?;

                        let installer_path =
                            installer_map.get(&batch.platform).cloned().ok_or_else(|| {
                                anyhow!(
                                    "no installer available for platform '{}'",
                                    batch.platform.as_str()
                                )
                            })?;

                        let job = plan::NativeBuild {
                            format,
                            image_name: name.clone(),
                            image: cfg.clone(),
                            platform: batch.platform,
                            script_path: script_path.to_path_buf(),
                            output_dir,
                            installer_path,
                            packaging_root: packaging_dir.to_path_buf(),
                            plan_rel,
                        };

                        let result = run_package(&runtime_clone, &prefix_clone, job).await?;
                        completed += result.len();
                        produced.extend(result);
                        counter.set(completed);
                    }
                }

                Ok(produced)
            },
            move |produced| {
                format!(
                    "Build native packages ({}/{})",
                    produced.len(),
                    package_count
                )
            },
        )
        .await?;

    push_download_summary(&mut final_messages, &download_summary);

    let rpm_count = results
        .iter()
        .filter(|result| result.format == PackageFormat::Rpm)
        .count();
    let deb_count = results
        .iter()
        .filter(|result| result.format == PackageFormat::Deb)
        .count();

    if rpm_count > 0 {
        final_messages.push(format!("Generated {rpm_count} RPM package(s)."));
    }
    if deb_count > 0 {
        final_messages.push(format!("Generated {deb_count} DEB package(s)."));
    }

    if !results.is_empty() {
        final_messages.push("Native package outputs:".to_string());
        for result in &results {
            final_messages.push(format!(
                "  - [{} {} {}] {}",
                result.format.label(),
                result.platform.as_str(),
                result.image_name,
                result.path.display()
            ));
        }
    }

    drop(progress);

    for message in final_messages {
        println!("{message}");
    }

    Ok(())
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

/// Deduplicate platforms while preserving their original order.
fn normalize_platforms(platforms: Vec<Platform>) -> Vec<Platform> {
    let mut seen = std::collections::HashSet::new();
    let mut ordered = Vec::new();
    for platform in platforms {
        if seen.insert(platform) {
            ordered.push(platform);
        }
    }
    ordered
}
