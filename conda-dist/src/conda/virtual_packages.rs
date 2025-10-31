use std::str::FromStr;

use anyhow::{Context, Result};
use rattler_conda_types::{GenericVirtualPackage, Platform, Version};
use rattler_virtual_packages::{Archspec, Cuda, LibC, Linux, Osx, VirtualPackages, Windows};

use crate::config::PlatformVirtualPackageConfig;

pub fn detect_virtual_packages_for_platform(
    platform: Platform,
    overrides: Option<&PlatformVirtualPackageConfig>,
) -> Result<Vec<GenericVirtualPackage>> {
    let mut packages = VirtualPackages::default();
    packages.unix = platform.is_unix();

    if platform.is_windows() {
        packages.win = Some(Windows {
            version: Some(parse_version(DEFAULT_WINDOWS_VERSION, "__win", platform)?),
        });
    }
    if platform.is_linux() {
        packages.linux = Some(Linux::from(parse_version(
            DEFAULT_LINUX_VERSION,
            "__linux",
            platform,
        )?));
        packages.libc = Some(LibC {
            family: "glibc".into(),
            version: parse_version(DEFAULT_GLIBC_VERSION, "__glibc", platform)?,
        });
    }
    if platform.is_osx() {
        packages.osx = Some(Osx::from(parse_version(
            DEFAULT_OSX_VERSION,
            "__osx",
            platform,
        )?));
    }
    packages.archspec = Archspec::from_platform(platform);

    apply_cross_platform_defaults(platform, &mut packages)?;

    if let Some(config) = overrides {
        apply_overrides(platform, &mut packages, config)?;
    }

    Ok(packages
        .into_virtual_packages()
        .map(GenericVirtualPackage::from)
        .collect())
}

fn apply_overrides(
    platform: Platform,
    packages: &mut VirtualPackages,
    config: &PlatformVirtualPackageConfig,
) -> Result<()> {
    if let Some(value) = config.linux.as_ref() {
        let version = parse_version(value, "__linux", platform)?;
        packages.linux = Some(Linux::from(version));
    }

    if let Some(value) = config.osx.as_ref() {
        let version = parse_version(value, "__osx", platform)?;
        packages.osx = Some(Osx::from(version));
    }

    if let Some(value) = config.win.as_ref() {
        if value.trim().is_empty() {
            packages.win = Some(Windows { version: None });
        } else {
            let version = parse_version(value, "__win", platform)?;
            packages.win = Some(Windows {
                version: Some(version),
            });
        }
    }

    if let Some(libc) = config.libc.as_ref() {
        let version = parse_version(&libc.version, "__glibc", platform)?;
        packages.libc = Some(LibC {
            family: libc.family.clone(),
            version,
        });
    }

    if let Some(value) = config.cuda.as_ref() {
        if value.trim().is_empty() {
            packages.cuda = None;
        } else {
            let version = parse_version(value, "__cuda", platform)?;
            packages.cuda = Some(Cuda::from(version));
        }
    }

    Ok(())
}

fn parse_version(value: &str, name: &str, platform: Platform) -> Result<Version> {
    Version::from_str(value).with_context(|| {
        format!(
            "failed to parse virtual package {} version '{}' for platform {}",
            name,
            value,
            platform.as_str()
        )
    })
}

fn apply_cross_platform_defaults(platform: Platform, packages: &mut VirtualPackages) -> Result<()> {
    if platform.is_linux() {
        if packages.linux.is_none() {
            packages.linux = Some(Linux::from(parse_version(
                DEFAULT_LINUX_VERSION,
                "__linux",
                platform,
            )?));
        }

        if packages.libc.is_none() {
            packages.libc = Some(LibC {
                family: "glibc".into(),
                version: parse_version(DEFAULT_GLIBC_VERSION, "__glibc", platform)?,
            });
        }
    }

    if platform.is_osx() && packages.osx.is_none() {
        packages.osx = Some(Osx::from(parse_version(
            DEFAULT_OSX_VERSION,
            "__osx",
            platform,
        )?));
    }

    if platform.is_windows() && packages.win.is_none() {
        packages.win = Some(Windows {
            version: Some(parse_version(DEFAULT_WINDOWS_VERSION, "__win", platform)?),
        });
    }

    Ok(())
}

const DEFAULT_LINUX_VERSION: &str = "5.4";
const DEFAULT_GLIBC_VERSION: &str = "2.17";
const DEFAULT_OSX_VERSION: &str = "13.0";
const DEFAULT_WINDOWS_VERSION: &str = "10";
