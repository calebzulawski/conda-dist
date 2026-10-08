//! Plan shared dependency payloads and separately installable metapackages.

use std::collections::HashSet;

use anyhow::{Result, bail};
use rattler_conda_types::{
    PackageName, PackageRecord, Platform, RepoDataRecord, VersionWithSource,
};

use super::{
    model::{DependencyPackage, bundle_capability, split_package_name},
    plan::{BasePackageMetadata, ManifestLayout, write_base_entry, write_dependency_entry},
};
use crate::{app::artifact_name, conda, config::CondaDistConfig};

pub(super) fn write_entries(
    layout: &ManifestLayout,
    config: &CondaDistConfig,
    prep: &crate::app::environment::EnvironmentPreparation,
    platform: Platform,
    metadata: &BasePackageMetadata<'_>,
    dependency_packages: &[DependencyPackage],
) -> Result<Vec<String>> {
    validate_package_names(config, dependency_packages)?;
    let capability = [bundle_capability(config.name())];
    let metadata = BasePackageMetadata {
        conflicts: &capability,
        ..*metadata
    };
    let env_name = config.name();
    let base_records = conda::locked_packages(&prep.lockfile, "default:base", platform)?;
    let base_names: HashSet<_> = base_records
        .iter()
        .map(|record| record.package_record.name.as_normalized())
        .collect();
    let mut lines = Vec::new();
    for component in
        std::iter::once(None).chain(config.components().keys().map(|name| Some(name.as_str())))
    {
        let name = artifact_name(env_name, component);
        let component_records = component
            .map(|component| {
                conda::locked_packages(
                    &prep.lockfile,
                    &format!("default:component:{component}"),
                    platform,
                )
            })
            .transpose()?;
        let records = component_records.as_ref().unwrap_or(&base_records);
        let (dependencies, provides) = component_dependencies(
            config,
            metadata.version,
            component,
            records,
            &base_names,
            dependency_packages,
        );
        let summary = match component {
            Some(component) => format!("{} ({component})", metadata.summary),
            None => metadata.summary.to_string(),
        };
        let component_metadata = BasePackageMetadata {
            env_name: &name,
            summary: &summary,
            dependencies: &dependencies,
            provides: &provides,
            ..metadata
        };
        lines.push(write_base_entry(layout, &component_metadata, platform)?);
    }
    lines.extend(write_subset_entries(layout, config, platform, &metadata)?);
    for dependency in dependency_packages {
        lines.push(write_dependency_entry(
            layout,
            dependency,
            env_name,
            metadata.author,
            metadata.prefix,
            metadata.license,
        )?);
    }
    Ok(lines)
}

fn write_subset_entries(
    layout: &ManifestLayout,
    config: &CondaDistConfig,
    platform: Platform,
    metadata: &BasePackageMetadata<'_>,
) -> Result<Vec<String>> {
    config
        .subsets()
        .iter()
        .map(|(subset_name, subset)| {
            let name = artifact_name(config.name(), Some(subset_name));
            let summary = format!("{} ({subset_name})", metadata.summary);
            let dependencies: Vec<_> = std::iter::once(None)
                .chain(subset.components.iter().map(|name| Some(name.as_str())))
                .map(|component| {
                    PackageRecord::new(
                        PackageName::new_unchecked(artifact_name(config.name(), component)),
                        metadata.version.clone(),
                        metadata.release.to_string(),
                    )
                })
                .collect();
            let subset_metadata = BasePackageMetadata {
                env_name: &name,
                summary: &summary,
                dependencies: &dependencies,
                ..*metadata
            };
            write_base_entry(layout, &subset_metadata, platform)
        })
        .collect()
}

fn validate_package_names(
    config: &CondaDistConfig,
    dependency_packages: &[DependencyPackage],
) -> Result<()> {
    let mut names: HashSet<_> = std::iter::once(config.name().to_string())
        .chain(
            config
                .subsets()
                .keys()
                .map(|name| artifact_name(config.name(), Some(name))),
        )
        .collect();
    names.extend(
        config
            .components()
            .keys()
            .map(|name| artifact_name(config.name(), Some(name))),
    );
    for dependency in dependency_packages {
        let name = split_package_name(
            config.name(),
            dependency.record.package_record.name.as_normalized(),
        );
        if !names.insert(name.clone()) {
            bail!("generated native package name collision: {name}");
        }
    }
    Ok(())
}

fn component_dependencies(
    config: &CondaDistConfig,
    version: &VersionWithSource,
    component: Option<&str>,
    records: &[RepoDataRecord],
    base_names: &HashSet<&str>,
    dependency_packages: &[DependencyPackage],
) -> (Vec<PackageRecord>, Vec<String>) {
    let payloads = dependency_packages.iter().filter(|dep| {
        let source = dep.record.package_record.name.as_normalized();
        records
            .iter()
            .any(|record| record.package_record.name.as_normalized() == source)
            && (component.is_none() || !base_names.contains(source))
    });
    let (mut dependencies, provides) = derive_base_deps(config.name(), payloads);
    if let Some(component) = component {
        dependencies.extend(
            std::iter::once(config.name().to_string())
                .chain(
                    config.components()[component]
                        .includes
                        .iter()
                        .map(|name| artifact_name(config.name(), Some(name))),
                )
                .map(|name| {
                    PackageRecord::new(
                        PackageName::new_unchecked(name),
                        version.clone(),
                        config.package_release().to_string(),
                    )
                }),
        );
    }
    (dependencies, provides)
}

fn derive_base_deps<'a>(
    env_name: &str,
    dependency_packages: impl IntoIterator<Item = &'a DependencyPackage>,
) -> (Vec<PackageRecord>, Vec<String>) {
    let mut deps = Vec::new();
    let mut provides = Vec::new();
    for dependency in dependency_packages {
        let name = split_package_name(
            env_name,
            dependency.record.package_record.name.as_normalized(),
        );
        deps.push(PackageRecord::new(
            PackageName::new_unchecked(name.clone()),
            dependency.record.package_record.version.clone(),
            dependency.native_build(),
        ));
        provides.push(format!("lock-{name}"));
    }
    (deps, provides)
}
