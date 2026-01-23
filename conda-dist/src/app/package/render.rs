//! Render RPM specs, DEB control files, and container helper scripts.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::filters;
use anyhow::Result;
use askama::Template;
use rattler_conda_types::Platform;

use super::{
    OUTPUT_DEST_PATH,
    model::{DependencyPackage, PackageFormat},
    plan::BasePackageMetadata,
};

const RPM_SCRIPT_NAME: &str = "package-rpm.sh";
const DEB_SCRIPT_NAME: &str = "package-deb.sh";
#[derive(Template)]
#[template(path = "rpm.spec.payload.txt", escape = "none")]
struct RpmSpecPayloadTemplate<'a> {
    name: &'a str,
    version: String,
    extra_build: String,
    summary: String,
    description_parts: Vec<String>,
    dependencies: Vec<rattler_conda_types::PackageRecord>,
    provides: Vec<String>,
    is_split: bool,
    lock_name: String,
    license: &'a str,
    prefix: &'a str,
    arch: &'a str,
}

#[derive(Template)]
#[template(path = "rpm.spec.nopayload.txt", escape = "none")]
struct RpmSpecNoPayloadTemplate<'a> {
    name: &'a str,
    version: String,
    extra_build: String,
    summary: String,
    description_parts: Vec<String>,
    dependencies: Vec<rattler_conda_types::PackageRecord>,
    provides: Vec<String>,
    is_split: bool,
    lock_name: String,
    license: &'a str,
    arch: &'a str,
}

#[derive(Template)]
#[template(path = "deb.control.txt", escape = "none")]
struct DebControlTemplate<'a> {
    name: &'a str,
    version: String,
    build: String,
    extra_build: String,
    summary: String,
    description_parts: Vec<String>,
    dependencies: Vec<rattler_conda_types::PackageRecord>,
    extra_depends: Vec<String>,
    provides: Vec<String>,
    author: &'a str,
    arch: &'a str,
}

/// Emit the container-side packaging script for the requested format.
pub fn write_script(format: PackageFormat, root: &Path) -> Result<PathBuf> {
    let (name, contents) = match format {
        PackageFormat::Rpm => (RPM_SCRIPT_NAME, include_str!("package-rpm.sh")),
        PackageFormat::Deb => (DEB_SCRIPT_NAME, include_str!("package-deb.sh")),
    };
    let path = root.join(name);
    let script = contents.replace("{OUTPUT_DEST_PATH}", OUTPUT_DEST_PATH);
    super::write_script(&path, &script)?;
    Ok(path)
}

/// Map a conda platform to the native package architecture string.
pub fn arch(format: PackageFormat, platform: Platform) -> Result<String> {
    Ok(match format {
        PackageFormat::Rpm => match platform {
            Platform::NoArch => "noarch",
            Platform::Linux64 => "x86_64",
            Platform::LinuxAarch64 => "aarch64",
            Platform::LinuxPpc64le => "ppc64le",
            Platform::LinuxS390X => "s390x",
            Platform::LinuxArmV7l => "armv7hl",
            Platform::Linux32 => "i686",
            other => unreachable!(
                "platform '{}' is not supported for RPM packaging",
                other.as_str()
            ),
        },
        PackageFormat::Deb => match platform {
            Platform::NoArch => "all",
            Platform::Linux64 => "amd64",
            Platform::LinuxAarch64 => "arm64",
            Platform::LinuxPpc64le => "ppc64el",
            Platform::LinuxS390X => "s390x",
            Platform::LinuxArmV7l => "armhf",
            Platform::Linux32 => "i386",
            other => unreachable!(
                "platform '{}' is not supported for DEB packaging",
                other.as_str()
            ),
        },
    }
    .to_string())
}

/// Render the RPM spec file for the base package.
pub(crate) fn write_base_rpm_spec(
    path: &Path,
    base: &BasePackageMetadata<'_>,
    platform: Platform,
) -> Result<()> {
    let arch = arch(PackageFormat::Rpm, platform)?;
    let is_split = false;
    let lock_name = String::new();
    let rendered = if base.base_full {
        RpmSpecPayloadTemplate {
            name: base.env_name,
            version: base.version.to_string(),
            extra_build: String::new(),
            summary: base.summary.to_string(),
            description_parts: base.description_parts.to_vec(),
            dependencies: base.dependencies.to_vec(),
            provides: base.provides.to_vec(),
            is_split,
            lock_name,
            license: base.license,
            prefix: base.prefix,
            arch: &arch,
        }
        .render()?
    } else {
        RpmSpecNoPayloadTemplate {
            name: base.env_name,
            version: base.version.to_string(),
            extra_build: String::new(),
            summary: base.summary.to_string(),
            description_parts: base.description_parts.to_vec(),
            dependencies: base.dependencies.to_vec(),
            provides: base.provides.to_vec(),
            is_split,
            lock_name,
            license: base.license,
            arch: &arch,
        }
        .render()?
    };
    std::fs::write(path, rendered)?;
    Ok(())
}

/// Render the RPM spec file for a dependency package.
pub(crate) fn write_sub_rpm_spec(
    path: &Path,
    sub: &DependencyPackage,
    name: &str,
    license: &str,
    prefix: &str,
) -> Result<()> {
    let subdir_platform = Platform::from_str(&sub.record.package_record.subdir)?;
    let arch = arch(PackageFormat::Rpm, subdir_platform)?;
    let (summary, description_parts) = sub_summary_parts(sub);
    let dependencies = Vec::new();
    let provides = Vec::new();
    let version = sub.record.package_record.version.to_string();
    let extra_build = sub_extra_build(sub);
    let is_split = true;
    let lock_name = format!("lock-{name}");
    let rendered = RpmSpecPayloadTemplate {
        name,
        version,
        extra_build,
        summary,
        description_parts,
        dependencies,
        provides,
        is_split,
        lock_name,
        license,
        prefix,
        arch: &arch,
    }
    .render()?;
    std::fs::write(path, rendered)?;
    Ok(())
}

/// Render the DEB control file for the base package.
pub(crate) fn write_base_deb_control(
    path: &Path,
    base: &BasePackageMetadata<'_>,
    platform: Platform,
) -> Result<()> {
    let arch = arch(PackageFormat::Deb, platform)?;
    let build = base.release.to_string();
    let control = DebControlTemplate {
        name: base.env_name,
        version: base.version.to_string(),
        build,
        extra_build: String::new(),
        summary: base.summary.to_string(),
        description_parts: base.description_parts.to_vec(),
        dependencies: base.dependencies.to_vec(),
        extra_depends: Vec::new(),
        provides: base.provides.to_vec(),
        author: base.author,
        arch: &arch,
    }
    .render()?;
    std::fs::write(path, format!("{control}\n"))?;
    Ok(())
}

/// Render the DEB control file for a dependency package.
pub(crate) fn write_sub_deb_control(
    path: &Path,
    sub: &DependencyPackage,
    name: &str,
    author: &str,
) -> Result<()> {
    let subdir_platform = Platform::from_str(&sub.record.package_record.subdir)?;
    let arch = arch(PackageFormat::Deb, subdir_platform)?;
    let (summary, description_parts) = sub_summary_parts(sub);
    let provides = Vec::new();
    let version = sub.record.package_record.version.to_string();
    let build = sub.record.package_record.build.clone();
    let extra_build = sub_extra_build(sub);
    let extra_depends = vec![format!("lock-{name}")];
    let control = DebControlTemplate {
        name,
        version,
        build,
        extra_build,
        summary,
        description_parts,
        dependencies: Vec::new(),
        extra_depends,
        provides,
        author,
        arch: &arch,
    }
    .render()?;
    std::fs::write(path, format!("{control}\n"))?;
    Ok(())
}

fn sub_summary_parts(sub: &DependencyPackage) -> (String, Vec<String>) {
    let summary = format!(
        "Conda package {}",
        sub.record.package_record.name.as_normalized()
    );
    let description_parts = vec![summary.clone()];
    (summary, description_parts)
}

fn sub_extra_build(sub: &DependencyPackage) -> String {
    sub.extra_build
        .as_ref()
        .map(|extra| format!("_{extra}"))
        .unwrap_or_default()
}
