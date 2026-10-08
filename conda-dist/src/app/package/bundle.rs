//! Plan complete environment packages for the base and subsets.

use anyhow::Result;
use rattler_conda_types::Platform;

use super::{
    model::bundle_capability,
    plan::{BasePackageMetadata, ManifestLayout, write_base_entry},
};
use crate::app::artifact_name;

pub(super) fn write_entry(
    layout: &ManifestLayout,
    platform: Platform,
    metadata: &BasePackageMetadata<'_>,
    subset: Option<&str>,
) -> Result<String> {
    let name = artifact_name(metadata.env_name, subset);
    let capability = [bundle_capability(metadata.env_name)];
    let summary = match subset {
        Some(subset) => format!("{} ({subset})", metadata.summary),
        None => metadata.summary.to_string(),
    };
    let metadata = BasePackageMetadata {
        env_name: &name,
        summary: &summary,
        provides: &capability,
        conflicts: &capability,
        ..*metadata
    };
    write_base_entry(layout, &metadata, platform)
}
