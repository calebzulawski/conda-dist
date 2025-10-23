use std::path::Path;

use anyhow::{Context, Result, bail};
use rattler::install::Installer;
use tokio::process::Command;

use crate::bundle::{BundleData, PostInstall};

pub async fn install(prefix: &Path, bundle: &BundleData, project_name: &str) -> Result<()> {
    let record_count = bundle.records.len();
    let cache_dir = tempfile::tempdir().context("failed to prepare temporary cache directory")?;
    let package_cache = rattler::package_cache::PackageCache::new(cache_dir.path());
    let installer = Installer::new()
        .with_target_platform(bundle.target_platform)
        .with_package_cache(package_cache);

    let result = installer
        .install(prefix, bundle.records.clone())
        .await
        .context("failed to complete the installation")?;

    let installed = result.transaction.packages_to_install();
    let removed = result.transaction.packages_to_uninstall();

    println!(
        "Installed {project} into {destination}",
        project = bundle.metadata.display_name,
        destination = prefix.display()
    );
    println!(
        "Applied {installs} updates and removed {removals} items (bundle contained {count} components for {platform})",
        installs = installed,
        removals = removed,
        count = record_count,
        platform = bundle.target_platform.as_str()
    );

    if let Some(post_install) = bundle.metadata.post_install.as_ref() {
        run_post_install(post_install, prefix, &bundle.channel_dir, project_name).await?;
    }

    if let Some(message) = bundle.metadata.success_message.as_deref() {
        println!("\n{message}");
    }

    cache_dir
        .close()
        .context("failed to clean up temporary cache directory")?;

    Ok(())
}

async fn run_post_install(
    post_install: &PostInstall,
    prefix: &Path,
    channel_dir: &Path,
    project_name: &str,
) -> Result<()> {
    let script_path = channel_dir.join(&post_install.script);
    println!("Running post-install script '{}'.", script_path.display());
    let mut command = Command::new(&script_path);
    command
        .env("CONDA_DIST_INSTALL_PREFIX", prefix)
        .env("CONDA_DIST_BUNDLE_DIR", channel_dir)
        .env("CONDA_DIST_PROJECT_NAME", project_name)
        .current_dir(prefix);

    let status = command.status().await.with_context(|| {
        format!(
            "failed to invoke post-install script {}",
            script_path.display()
        )
    })?;

    if !status.success() {
        bail!("post-install script exited with status {}", status);
    }

    Ok(())
}
