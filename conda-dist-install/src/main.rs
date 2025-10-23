use std::{
    env,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use clap::{FromArgMatches, Parser};
use rattler::install::Installer;
use rattler_conda_types::{Platform, RepoDataRecord};
use rattler_lock::{CondaPackageData, DEFAULT_ENVIRONMENT_NAME, LockFile};
use url::Url;

const DEFAULT_LOCKFILE_NAME: &str = "conda-lock.yml";

#[derive(Debug, Parser)]
#[command(version, disable_help_subcommand = true)]
struct Cli {
    /// Destination path for the installation
    #[arg(value_name = "INSTALLATION_PATH")]
    prefix: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let project_name =
        env::var("CONDA_DIST_PROJECT_NAME").unwrap_or_else(|_| "this project".to_string());
    let cli = parse_cli(&project_name)?;
    execute(cli, &project_name).await
}

fn parse_cli(project_name: &str) -> Result<Cli> {
    use clap::CommandFactory;

    let mut command = Cli::command();
    let about = format!("Install {project}", project = project_name);
    let about: &'static str = Box::leak(about.into_boxed_str());
    command = command.about(about);

    let matches = match command.try_get_matches() {
        Ok(matches) => matches,
        Err(err) => err.exit(),
    };

    match Cli::from_arg_matches(&matches) {
        Ok(cli) => Ok(cli),
        Err(err) => err.exit(),
    }
}

async fn execute(cli: Cli, project_name: &str) -> Result<()> {
    let bundle_dir = env::var("CONDA_DIST_BUNDLE_DIR").context(
        "installer bundle was not initialised correctly; please re-run the launcher script.",
    )?;

    let channel_dir = PathBuf::from(&bundle_dir)
        .canonicalize()
        .with_context(|| format!("failed to resolve installer bundle at {bundle_dir}"))?;

    let lockfile_path = locate_lockfile(&channel_dir)?;
    let lockfile = LockFile::from_path(&lockfile_path)
        .with_context(|| format!("failed to read lockfile at {}", lockfile_path.display()))?;

    let (_, environment) = resolve_environment(&lockfile, None)?;

    let target_platform = Platform::current();

    let records = collect_records(environment, target_platform, &channel_dir)?;

    if records.is_empty() {
        bail!(
            "the installer bundle for '{project}' does not contain components for {platform}",
            project = project_name,
            platform = target_platform.as_str()
        );
    }

    let record_count = records.len();
    let installer = Installer::new().with_target_platform(target_platform);
    let result = installer
        .install(&cli.prefix, records)
        .await
        .context("failed to complete the installation")?;

    let installed = result.transaction.packages_to_install();
    let removed = result.transaction.packages_to_uninstall();

    println!(
        "Installed {project} into {destination}",
        project = project_name,
        destination = cli.prefix.display()
    );
    println!(
        "Applied {installs} updates and removed {removals} items (bundle contained {count} components for {platform})",
        installs = installed,
        removals = removed,
        count = record_count,
        platform = target_platform.as_str()
    );

    Ok(())
}

fn locate_lockfile(channel_dir: &Path) -> Result<PathBuf> {
    let candidate = channel_dir.join(DEFAULT_LOCKFILE_NAME);
    if candidate.exists() {
        return Ok(candidate.canonicalize().with_context(|| {
            format!("failed to resolve lockfile path {}", candidate.display())
        })?);
    }

    bail!(
        "installer resources are incomplete (missing descriptor at {}). Please re-download the installer.",
        candidate.display()
    );
}

fn resolve_environment<'lock>(
    lockfile: &'lock LockFile,
    requested: Option<&str>,
) -> Result<(String, rattler_lock::Environment<'lock>)> {
    if let Some(name) = requested {
        let environment = lockfile
            .environment(name)
            .with_context(|| format!("installer definition '{name}' not found in bundle"))?;
        return Ok((name.to_string(), environment));
    }

    if let Some(environment) = lockfile.default_environment() {
        return Ok((DEFAULT_ENVIRONMENT_NAME.to_string(), environment));
    }

    let environments: Vec<_> = lockfile.environments().collect();
    match environments.as_slice() {
        [] => bail!("installer bundle did not contain any installation definitions"),
        [(name, environment)] => Ok(((*name).to_string(), *environment)),
        _ => bail!("installer bundle is ambiguous; contact the package author for assistance"),
    }
}

fn collect_records(
    environment: rattler_lock::Environment<'_>,
    target_platform: Platform,
    channel_dir: &Path,
) -> Result<Vec<RepoDataRecord>> {
    let mut records = Vec::new();
    let mut has_target_platform = false;

    for (platform, packages) in environment.conda_packages_by_platform() {
        if platform == target_platform {
            has_target_platform = true;
        }

        if platform != target_platform && platform != Platform::NoArch {
            continue;
        }

        for package in packages {
            match package {
                CondaPackageData::Binary(data) => {
                    let package_path = channel_dir
                        .join(&data.package_record.subdir)
                        .join(&data.file_name);
                    if !package_path.exists() {
                        bail!(
                            "installer component missing from bundle: {}",
                            package_path.display()
                        );
                    }

                    let url = Url::from_file_path(&package_path).map_err(|_| {
                        anyhow!(
                            "failed to prepare installer component {}",
                            package_path.display()
                        )
                    })?;

                    let record = RepoDataRecord {
                        package_record: data.package_record.clone(),
                        file_name: data.file_name.clone(),
                        url,
                        channel: Some("local".to_string()),
                    };
                    records.push(record);
                }
                CondaPackageData::Source(data) => {
                    bail!(
                        "installer component '{}' is not supported on this system",
                        data.package_record.name.as_source()
                    );
                }
            }
        }
    }

    if !has_target_platform {
        bail!(
            "the installer bundle does not include components for platform {}",
            target_platform.as_str()
        );
    }

    Ok(records)
}
