//! Generate package_plan.tsv entries and layout for containerized package builds.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use rattler_conda_types::Platform;

use super::{
    bundle,
    model::{DependencyPackage, PackageMode, install_prefix, split_package_name},
    modular, render,
};
use crate::config::PackageFormat;

#[derive(Debug, Clone)]
pub struct NativeBuild {
    pub format: PackageFormat,
    pub image_name: String,
    pub image: crate::config::PackageImageConfig,
    pub platform: Platform,
    pub script_path: PathBuf,
    pub output_dir: PathBuf,
    pub installer_path: PathBuf,
    pub packaging_root: PathBuf,
    pub plan_rel: PathBuf,
    pub subset: Option<String>,
    pub mode: PackageMode,
}

pub(super) struct ManifestLayout {
    packaging_root: PathBuf,
    root: PathBuf,
    format: PackageFormat,
    specs_dir: PathBuf,
    controls_dir: PathBuf,
    topdir_dir: PathBuf,
    roots_dir: PathBuf,
    lists_dir: PathBuf,
}

impl ManifestLayout {
    fn new(
        packaging_root: &Path,
        mode: PackageMode,
        format: PackageFormat,
        platform: Platform,
        image_name: &str,
    ) -> Self {
        let root = packaging_root
            .join(mode.label())
            .join(format.label())
            .join(platform.as_str())
            .join(image_name);
        let specs_dir = root.join("specs");
        let controls_dir = root.join("controls");
        let topdir_dir = root.join("topdir");
        let roots_dir = root.join("roots");
        let lists_dir = root.join("lists");
        Self {
            packaging_root: packaging_root.to_path_buf(),
            root,
            format,
            specs_dir,
            controls_dir,
            topdir_dir,
            roots_dir,
            lists_dir,
        }
    }

    fn ensure_dirs(&self) -> Result<()> {
        for dir in [
            &self.specs_dir,
            &self.controls_dir,
            &self.topdir_dir,
            &self.roots_dir,
            &self.lists_dir,
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    fn plan_path(&self) -> PathBuf {
        self.root.join("package_plan.tsv")
    }

    fn relative_path(&self, path: &Path) -> Result<PathBuf> {
        path.strip_prefix(&self.packaging_root)
            .map(Path::to_path_buf)
            .map_err(|_| {
                anyhow!(
                    "path {} is not under {}",
                    path.display(),
                    self.packaging_root.display()
                )
            })
    }
}

/// Describe base package metadata used for rendering RPM/DEB control data.
pub struct BasePackageMetadata<'a> {
    pub env_name: &'a str,
    pub author: &'a str,
    pub prefix: &'a str,
    pub license: &'a str,
    pub release: &'a str,
    pub version: &'a rattler_conda_types::VersionWithSource,
    pub summary: &'a str,
    pub description_parts: &'a [String],
    pub dependencies: &'a [rattler_conda_types::PackageRecord],
    pub provides: &'a [String],
    pub base_full: bool,
    pub conflicts: &'a [String],
}

pub struct PackagePlanInput<'a> {
    pub mode: PackageMode,
    pub subset: Option<&'a str>,
    pub dependency_packages: &'a [DependencyPackage],
}

/// Write metadata for a bundle or a complete modular package family.
pub fn write_package_plan(
    format: PackageFormat,
    image_name: &str,
    platform: Platform,
    packaging_root: &Path,
    input: PackagePlanInput<'_>,
    manifest_ctx: &crate::app::context::ManifestContext,
    prep: &crate::app::environment::EnvironmentPreparation,
) -> Result<PathBuf> {
    let config = &manifest_ctx.config;
    let layout = ManifestLayout::new(packaging_root, input.mode, format, platform, image_name);
    layout.ensure_dirs()?;
    let env_name = prep.environment_name.as_str();
    let author = super::model::package_author(manifest_ctx);
    let prefix = install_prefix(manifest_ctx, prep)?;
    let version = super::model::base_version(manifest_ctx)?;
    let description = super::model::base_description_parts(prep);
    let base_summary = super::model::base_summary(prep)?;
    let metadata = BasePackageMetadata {
        env_name,
        author: &author,
        prefix: &prefix,
        license: config.license(),
        release: config.package_release(),
        version: &version,
        summary: &base_summary,
        description_parts: &description,
        dependencies: &[],
        provides: &[],
        base_full: input.mode == PackageMode::Bundle,
        conflicts: &[],
    };
    let lines = match input.mode {
        PackageMode::Bundle => vec![bundle::write_entry(
            &layout,
            platform,
            &metadata,
            input.subset,
        )?],
        PackageMode::Modular => modular::write_entries(
            &layout,
            config,
            prep,
            platform,
            &metadata,
            input.dependency_packages,
        )?,
    };
    let plan_path = layout.plan_path();
    std::fs::write(&plan_path, format!("{}\n", lines.join("\n")))?;
    layout.relative_path(&plan_path)
}

/// Render the base package entry and return its package_plan.tsv line.
pub(super) fn write_base_entry(
    layout: &ManifestLayout,
    ctx: &BasePackageMetadata<'_>,
    platform: Platform,
) -> Result<String> {
    let base_payload_mode = if ctx.base_full { "full" } else { "none" };
    match layout.format {
        PackageFormat::Rpm => {
            let spec_path = layout.specs_dir.join(format!("{}.spec", ctx.env_name));
            render::write_base_rpm_spec(&spec_path, ctx, platform)?;
            let topdir = layout.topdir_dir.clone();
            Ok(format!(
                "{}\t{}\t{}\t{}\t{}\t-",
                ctx.env_name,
                base_payload_mode,
                layout.relative_path(&spec_path)?.display(),
                layout.relative_path(&topdir)?.display(),
                crate::filters::normalize_rpm_release(ctx.release)
            ))
        }
        PackageFormat::Deb => {
            let control_path = layout
                .controls_dir
                .join(format!("{}.control", ctx.env_name));
            render::write_base_deb_control(&control_path, ctx, platform)?;
            let root = layout.roots_dir.join(ctx.env_name);
            Ok(format!(
                "{}\t{}\t{}\t{}\t-\t-",
                ctx.env_name,
                base_payload_mode,
                layout.relative_path(&control_path)?.display(),
                layout.relative_path(&root)?.display(),
            ))
        }
    }
}

/// Render a dependency package entry and return its package_plan.tsv line.
pub(super) fn write_dependency_entry(
    layout: &ManifestLayout,
    dependency: &DependencyPackage,
    env_name: &str,
    author: &str,
    prefix: &str,
    license: &str,
) -> Result<String> {
    let package_name = split_package_name(
        env_name,
        dependency.record.package_record.name.as_normalized(),
    );
    let payload_mode = "files";
    let list_path = layout.lists_dir.join(format!("{package_name}.list"));
    write_filelist(&list_path, prefix, &dependency.files)?;
    let filelist_rel = layout.relative_path(&list_path)?;

    match layout.format {
        PackageFormat::Rpm => {
            let spec_path = layout.specs_dir.join(format!("{package_name}.spec"));
            render::write_sub_rpm_spec(&spec_path, dependency, &package_name, license, prefix)?;
            let topdir = layout.topdir_dir.clone();
            Ok(format!(
                "{}\t{}\t{}\t{}\t{}\t{}",
                package_name,
                payload_mode,
                layout.relative_path(&spec_path)?.display(),
                layout.relative_path(&topdir)?.display(),
                crate::filters::normalize_rpm_release(&dependency.native_build()),
                filelist_rel.display()
            ))
        }
        PackageFormat::Deb => {
            let control_path = layout.controls_dir.join(format!("{package_name}.control"));
            render::write_sub_deb_control(&control_path, dependency, &package_name, author)?;
            let root = layout.roots_dir.join(&package_name);
            Ok(format!(
                "{}\t{}\t{}\t{}\t-\t{}",
                package_name,
                payload_mode,
                layout.relative_path(&control_path)?.display(),
                layout.relative_path(&root)?.display(),
                filelist_rel.display()
            ))
        }
    }
}

/// Write a payload file list rooted at the install prefix for container extraction.
fn write_filelist(path: &Path, prefix: &str, files: &[PathBuf]) -> Result<()> {
    let prefix = Path::new(prefix.trim_start_matches('/'));
    let mut lines = Vec::new();
    for entry in files {
        let full = prefix.join(entry);
        lines.push(full.to_string_lossy().to_string());
    }
    std::fs::write(path, format!("{}\n", lines.join("\n")))?;
    Ok(())
}
