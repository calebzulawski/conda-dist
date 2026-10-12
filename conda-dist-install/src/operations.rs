use std::path::Path;

use anyhow::{Context, Result, bail};
use rattler::install::Installer;

use crate::bundle::BundleData;

pub async fn install(
    prefix: &Path,
    bundle: &BundleData,
    components: Option<&[String]>,
) -> Result<()> {
    match std::fs::symlink_metadata(prefix) {
        Ok(_) => bail!(
            "installation destination {} already exists",
            prefix.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).context(format!(
                "failed to inspect installation destination {}",
                prefix.display()
            ));
        }
    }
    let records = bundle.records_for_components(components)?;
    let record_count = records.len();
    let cache_dir = tempfile::tempdir().context("failed to prepare temporary cache directory")?;
    let package_cache = rattler::package_cache::PackageCache::new(cache_dir.path());
    let installer = Installer::new()
        .with_target_platform(bundle.target_platform)
        .with_package_cache(package_cache);

    let result = installer
        .install(prefix, records)
        .await
        .context("failed to complete the installation")?;

    let installed = result.transaction.packages_to_install();
    let removed = result.transaction.packages_to_uninstall();

    println!(
        "Installed {project} into {destination}",
        project = bundle.metadata.summary,
        destination = prefix.display()
    );
    println!("Maintainer: {}", bundle.metadata.author);
    println!(
        "Applied {installs} updates and removed {removals} items (selection contained {count} packages for {platform})",
        installs = installed,
        removals = removed,
        count = record_count,
        platform = bundle.target_platform.as_str()
    );

    cache_dir
        .close()
        .context("failed to clean up temporary cache directory")?;

    Ok(())
}
