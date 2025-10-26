use anyhow::Result;
use rattler_conda_types::GenericVirtualPackage;
use rattler_virtual_packages::{VirtualPackage, VirtualPackageOverrides};

pub fn detect_virtual_packages() -> Result<Vec<GenericVirtualPackage>> {
    let overrides = VirtualPackageOverrides::default();
    let packages = VirtualPackage::detect(&overrides)?
        .into_iter()
        .map(GenericVirtualPackage::from)
        .collect();
    Ok(packages)
}
