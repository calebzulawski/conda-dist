use std::{
    collections::{HashMap, HashSet},
    env, fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine as _, engine::general_purpose};
use rattler_conda_types::Platform;
use tokio::process::Command;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use crate::{cli::PackageArgs, installer, progress::Progress, workspace::Workspace};

use super::{
    LockMode,
    context::{ManifestContext, load_manifest_context},
    environment::{EnvironmentPreparation, prepare_environment},
    push_download_summary,
    runtime::{self, RuntimeBinary, RuntimeEngine},
};

const DEFAULT_RELEASE: &str = "1";
const DEFAULT_LICENSE: &str = "Proprietary";
const DEFAULT_DEB_SECTION: &str = "misc";
const DEFAULT_DEB_PRIORITY: &str = "optional";
const RPM_SCRIPT_NAME: &str = "package-rpm.sh";
const DEB_SCRIPT_NAME: &str = "package-deb.sh";
const SCRIPT_DEST_PATH: &str = "/tmp/conda-dist-package.sh";
const INSTALLER_MOUNT_ROOT: &str = "/input";
const OUTPUT_DEST_PATH: &str = "/output";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PackageFormat {
    Rpm,
    Deb,
}

impl PackageFormat {
    fn label(self) -> &'static str {
        match self {
            Self::Rpm => "rpm",
            Self::Deb => "deb",
        }
    }
}

#[derive(Debug, Clone)]
struct PackageJob {
    format: PackageFormat,
    image: String,
    platform: Platform,
    installer_path: PathBuf,
    script_path: PathBuf,
    output_dir: PathBuf,
    arch: String,
}

#[derive(Debug)]
struct PackageResult {
    format: PackageFormat,
    image: String,
    platform: Platform,
    path: PathBuf,
}

#[derive(Debug, Clone)]
struct PackageMetadata {
    name: String,
    version: String,
    author: String,
    prefix: String,
    release: String,
    license: String,
    summary_b64: String,
    description_b64: String,
    deb_section: String,
    deb_priority: String,
}

impl PackageMetadata {
    fn from_manifest(
        manifest_ctx: &ManifestContext,
        prep: &EnvironmentPreparation,
    ) -> Result<Self> {
        let name = prep.environment_name.clone();
        let version = manifest_ctx.config.version().trim().to_string();

        let author_line = sanitize_single_line(manifest_ctx.config.author());

        let prefix = manifest_ctx
            .config
            .container()
            .and_then(|cfg| cfg.prefix.clone())
            .unwrap_or_else(|| format!("/opt/{name}"));
        if !prefix.starts_with('/') {
            bail!(
                "package prefix '{prefix}' must be an absolute path; update container.prefix or specify a fully-qualified path"
            );
        }

        let summary_source = prep.bundle_metadata.manifest.summary.trim();
        let summary_line = if summary_source.is_empty() {
            name.clone()
        } else {
            sanitize_single_line(summary_source)
        };
        if summary_line.is_empty() {
            bail!("package summary for native builds must not be empty");
        }

        let description_text = compose_description(&prep.bundle_metadata.manifest);

        Ok(Self {
            name,
            version,
            author: author_line,
            prefix,
            release: DEFAULT_RELEASE.to_string(),
            license: DEFAULT_LICENSE.to_string(),
            summary_b64: encode_b64(&summary_line),
            description_b64: encode_b64(&description_text),
            deb_section: DEFAULT_DEB_SECTION.to_string(),
            deb_priority: DEFAULT_DEB_PRIORITY.to_string(),
        })
    }
}

pub async fn execute(
    args: PackageArgs,
    work_dir: Option<PathBuf>,
    lock_mode: LockMode,
) -> Result<()> {
    let PackageArgs {
        manifest,
        engine,
        rpm_images,
        deb_images,
        platform,
        output_dir,
    } = args;

    if rpm_images.is_empty() && deb_images.is_empty() {
        bail!("at least one --rpm-image or --deb-image must be provided");
    }

    let manifest_ctx = load_manifest_context(manifest)?;
    let workspace = Workspace::from_manifest_dir(&manifest_ctx.manifest_dir, work_dir)?;
    let runtime = runtime::resolve_runtime(engine)?;

    let mut requested_platforms: Vec<Platform> = if platform.is_empty() {
        vec![Platform::current()]
    } else {
        platform
    };

    let mut seen = HashSet::new();
    requested_platforms.retain(|platform| seen.insert(*platform));

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

    let installer_platforms = prep.target_platforms.clone();
    let installer_summary = runtime::format_platform_list(&installer_platforms);
    let installer_label = format!("Prepare installer bundle [{installer_summary}]");
    let installer_step = progress.step(installer_label.clone());
    let installer_dir = prep.staging_dir.path().join("installers");
    fs::create_dir_all(&installer_dir).with_context(|| {
        format!(
            "failed to prepare installer staging directory {}",
            installer_dir.display()
        )
    })?;
    let prep_ref = &prep;
    let installer_platforms_for_task = installer_platforms.clone();
    let total_installers = installer_platforms_for_task.len();
    let installer_paths = installer_step
        .run_with(
            Some(Duration::from_millis(120)),
            {
                move |handle| async move {
                    let mut counter = handle.counter(total_installers);
                    installer::create_installers(
                        &installer_dir,
                        &prep_ref.environment_name,
                        &prep_ref.channel_dir,
                        &installer_platforms_for_task,
                        &prep_ref.bundle_metadata,
                        &mut counter,
                    )
                }
            },
            move |_| installer_label.clone(),
        )
        .await?;

    if installer_paths.len() != installer_platforms.len() {
        bail!(
            "unexpected installer output; expected {} artifacts but received {}",
            installer_platforms.len(),
            installer_paths.len()
        );
    }

    let metadata = PackageMetadata::from_manifest(&manifest_ctx, &prep)?;

    let output_root = match output_dir {
        Some(path) => env::current_dir()?.join(path),
        None => manifest_ctx.manifest_dir.clone(),
    };
    fs::create_dir_all(&output_root).with_context(|| {
        format!(
            "failed to prepare native package output directory {}",
            output_root.display()
        )
    })?;

    let packaging_root = workspace.native_packaging_dir();
    let packaging_dir = packaging_root.join(&prep.environment_name);
    if packaging_dir.exists() {
        fs::remove_dir_all(&packaging_dir).with_context(|| {
            format!(
                "failed to clear previous packaging directory {}",
                packaging_dir.display()
            )
        })?;
    }
    fs::create_dir_all(&packaging_dir).with_context(|| {
        format!(
            "failed to prepare packaging directory {}",
            packaging_dir.display()
        )
    })?;

    let mut installer_by_platform: HashMap<Platform, PathBuf> = HashMap::new();
    for (platform, original_path) in installer_platforms
        .iter()
        .copied()
        .zip(installer_paths.into_iter())
    {
        let file_name = original_path.file_name().ok_or_else(|| {
            anyhow!(
                "installer path {} does not contain a valid file name",
                original_path.display()
            )
        })?;
        let dest_path = packaging_dir.join(file_name);
        if dest_path.exists() {
            fs::remove_file(&dest_path).with_context(|| {
                format!(
                    "failed to remove existing installer copy {}",
                    dest_path.display()
                )
            })?;
        }
        fs::copy(&original_path, &dest_path).with_context(|| {
            format!(
                "failed to copy installer {} into packaging directory",
                original_path.display()
            )
        })?;
        #[cfg(unix)]
        {
            let mut perms = fs::metadata(&dest_path)
                .with_context(|| {
                    format!("failed to inspect permissions for {}", dest_path.display())
                })?
                .permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&dest_path, perms).with_context(|| {
                format!(
                    "failed to mark installer copy {} as executable",
                    dest_path.display()
                )
            })?;
        }
        #[cfg(not(unix))]
        {
            let mut perms = fs::metadata(&dest_path)
                .with_context(|| {
                    format!("failed to inspect permissions for {}", dest_path.display())
                })?
                .permissions();
            perms.set_readonly(false);
            fs::set_permissions(&dest_path, perms).with_context(|| {
                format!(
                    "failed to update permissions on installer copy {}",
                    dest_path.display()
                )
            })?;
        }
        installer_by_platform.insert(platform, dest_path);
    }

    let rpm_script = if rpm_images.is_empty() {
        None
    } else {
        Some(write_rpm_script(&packaging_dir)?)
    };
    let deb_script = if deb_images.is_empty() {
        None
    } else {
        Some(write_deb_script(&packaging_dir)?)
    };

    let mut jobs = Vec::new();

    for platform in &installer_platforms {
        let installer_path = installer_by_platform
            .get(platform)
            .cloned()
            .ok_or_else(|| {
                anyhow!(
                    "installer for platform '{}' was not generated",
                    platform.as_str()
                )
            })?;

        if let Some(script_path) = rpm_script.as_ref() {
            let ctx = JobContext::new(*platform, &installer_path, script_path, &output_root);
            enqueue_package_jobs(PackageFormat::Rpm, &mut jobs, &ctx, &rpm_images, |plat| {
                rpm_arch(plat).map(|value| value.to_string())
            })?;
        }

        if let Some(script_path) = deb_script.as_ref() {
            let ctx = JobContext::new(*platform, &installer_path, script_path, &output_root);
            enqueue_package_jobs(PackageFormat::Deb, &mut jobs, &ctx, &deb_images, |plat| {
                deb_arch(plat).map(|value| value.to_string())
            })?;
        }
    }

    let job_count = jobs.len();
    if job_count == 0 {
        bail!("no native package jobs were scheduled");
    }

    let packaging_step = progress.step("Build native packages");
    let runtime_clone = runtime.clone();
    let metadata_clone = metadata.clone();

    let results = packaging_step
        .run_with(
            Some(Duration::from_millis(120)),
            move |handle| async move {
                let mut counter = handle.counter(job_count);

                let mut produced = Vec::new();
                for (index, job) in jobs.into_iter().enumerate() {
                    let result = run_package_job(&runtime_clone, &metadata_clone, job).await?;
                    produced.push(result);
                    counter.set(index + 1);
                }

                Ok(produced)
            },
            move |produced| format!("Build native packages ({}/{})", produced.len(), job_count),
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
                result.image,
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

async fn run_package_job(
    runtime: &RuntimeBinary,
    metadata: &PackageMetadata,
    job: PackageJob,
) -> Result<PackageResult> {
    let PackageJob {
        format,
        image,
        platform,
        installer_path,
        script_path,
        output_dir,
        arch,
    } = job;

    if !installer_path.exists() {
        bail!(
            "installer artifact missing at {}; rerun the build",
            installer_path.display()
        );
    }

    let installer_parent = installer_path.parent().ok_or_else(|| {
        anyhow!(
            "installer path {} has no parent directory",
            installer_path.display()
        )
    })?;

    let installer_name = installer_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            anyhow!(
                "installer path {} does not contain a valid UTF-8 file name",
                installer_path.display()
            )
        })?;

    let mut cmd = Command::new(runtime.binary());
    cmd.arg("run").arg("--rm");
    if matches!(
        runtime.engine(),
        RuntimeEngine::Docker | RuntimeEngine::Podman
    ) {
        let spec = runtime::platform_to_runtime_spec(platform)?;
        cmd.arg("--platform").arg(spec);
    }
    cmd.arg("--mount").arg(format!(
        "type=bind,src={},dst={},ro",
        installer_parent.display(),
        INSTALLER_MOUNT_ROOT
    ));
    cmd.arg("--mount").arg(format!(
        "type=bind,src={},dst={},ro",
        script_path.display(),
        SCRIPT_DEST_PATH
    ));
    cmd.arg("--mount").arg(format!(
        "type=bind,src={},dst={}",
        output_dir.display(),
        OUTPUT_DEST_PATH
    ));

    cmd.arg("--env").arg(format!("PKG_NAME={}", metadata.name));
    cmd.arg("--env")
        .arg(format!("PKG_VERSION={}", metadata.version));
    cmd.arg("--env")
        .arg(format!("PKG_AUTHOR={}", metadata.author));
    cmd.arg("--env")
        .arg(format!("PKG_PREFIX={}", metadata.prefix));
    cmd.arg("--env")
        .arg(format!("PKG_RELEASE={}", metadata.release));
    cmd.arg("--env")
        .arg(format!("PKG_LICENSE={}", metadata.license));
    cmd.arg("--env")
        .arg(format!("PKG_SUMMARY_B64={}", metadata.summary_b64));
    cmd.arg("--env")
        .arg(format!("PKG_DESCRIPTION_B64={}", metadata.description_b64));
    cmd.arg("--env").arg(format!(
        "PKG_INSTALLER_PATH={INSTALLER_MOUNT_ROOT}/{installer_name}"
    ));

    match format {
        PackageFormat::Rpm => {
            cmd.arg("--env").arg(format!("PKG_RPM_ARCH={arch}"));
        }
        PackageFormat::Deb => {
            cmd.arg("--env").arg(format!("PKG_DEB_ARCH={arch}"));
            cmd.arg("--env")
                .arg(format!("PKG_SECTION={}", metadata.deb_section));
            cmd.arg("--env")
                .arg(format!("PKG_PRIORITY={}", metadata.deb_priority));
        }
    }

    cmd.arg(&image);
    cmd.arg("/bin/bash").arg(SCRIPT_DEST_PATH);

    let start_time = std::time::SystemTime::now();
    runtime::run_command(&mut cmd, "package build").await?;

    let candidates = collect_new_artifacts(&output_dir, start_time)?;
    if candidates.is_empty() {
        bail!(
            "container '{}' completed but did not produce any new artifact in {}",
            image,
            output_dir.display()
        );
    }
    if candidates.len() > 1 {
        bail!(
            "container '{}' produced multiple artifacts in {}; expected exactly one",
            image,
            output_dir.display()
        );
    }
    let output_path = candidates.into_iter().next().unwrap();

    Ok(PackageResult {
        format,
        image,
        platform,
        path: output_path,
    })
}

fn collect_new_artifacts(output_dir: &Path, start_time: SystemTime) -> Result<Vec<PathBuf>> {
    let mut candidates = Vec::new();
    let mut newest: Option<(SystemTime, PathBuf)> = None;

    if !output_dir.exists() {
        return Ok(candidates);
    }

    for entry in fs::read_dir(output_dir).with_context(|| {
        format!(
            "failed to inspect output directory {}",
            output_dir.display()
        )
    })? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if !file_type.is_file() {
            continue;
        }
        let path = entry.path();
        let metadata = entry.metadata().with_context(|| {
            format!(
                "failed to inspect metadata for output artifact {}",
                path.display()
            )
        })?;
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if modified >= start_time {
            candidates.push(path.clone());
        }
        if newest
            .as_ref()
            .map(|(time, _)| modified > *time)
            .unwrap_or(true)
        {
            newest = Some((modified, path));
        }
    }

    if candidates.is_empty()
        && let Some((modified, path)) = newest
    {
        if modified >= start_time {
            candidates.push(path);
        } else if let Ok(diff) = start_time.duration_since(modified)
            && diff <= Duration::from_secs(2)
        {
            candidates.push(path);
        }
    }

    Ok(candidates)
}

fn rpm_arch(platform: Platform) -> Result<&'static str> {
    match platform {
        Platform::Linux64 => Ok("x86_64"),
        Platform::LinuxAarch64 => Ok("aarch64"),
        Platform::LinuxPpc64le => Ok("ppc64le"),
        Platform::LinuxS390X => Ok("s390x"),
        Platform::LinuxArmV7l => Ok("armv7hl"),
        Platform::Linux32 => Ok("i686"),
        other => unreachable!(
            "platform '{}' is not supported for RPM packaging",
            other.as_str()
        ),
    }
}

fn deb_arch(platform: Platform) -> Result<&'static str> {
    match platform {
        Platform::Linux64 => Ok("amd64"),
        Platform::LinuxAarch64 => Ok("arm64"),
        Platform::LinuxPpc64le => Ok("ppc64el"),
        Platform::LinuxS390X => Ok("s390x"),
        Platform::LinuxArmV7l => Ok("armhf"),
        Platform::Linux32 => Ok("i386"),
        other => unreachable!(
            "platform '{}' is not supported for DEB packaging",
            other.as_str()
        ),
    }
}

fn sanitize_single_line(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn compose_description(manifest: &installer::BundleMetadataManifest) -> String {
    let mut sections = Vec::new();
    if let Some(desc) = manifest.description.as_ref() {
        let trimmed = desc.trim();
        if !trimmed.is_empty() {
            sections.push(trimmed.to_string());
        }
    }
    if let Some(notes) = manifest.release_notes.as_ref() {
        let trimmed = notes.trim();
        if !trimmed.is_empty() {
            sections.push(format!("Release notes:\n{trimmed}"));
        }
    }
    sections.join("\n\n")
}

fn encode_b64(value: &str) -> String {
    general_purpose::STANDARD.encode(value.as_bytes())
}

fn sanitize_image_label(image: &str) -> String {
    let mut label = String::new();
    let mut last_sep = false;
    for ch in image.chars() {
        if ch.is_ascii_alphanumeric() {
            label.push(ch.to_ascii_lowercase());
            last_sep = false;
        } else if !last_sep {
            label.push('_');
            last_sep = true;
        }
    }
    let trimmed = label.trim_matches('_');
    if trimmed.is_empty() {
        "image".to_string()
    } else {
        trimmed.to_string()
    }
}

struct JobContext<'a> {
    platform: Platform,
    installer_path: &'a Path,
    script_path: &'a Path,
    output_root: &'a Path,
}

impl<'a> JobContext<'a> {
    fn new(
        platform: Platform,
        installer_path: &'a Path,
        script_path: &'a Path,
        output_root: &'a Path,
    ) -> Self {
        Self {
            platform,
            installer_path,
            script_path,
            output_root,
        }
    }
}

fn enqueue_package_jobs<F>(
    format: PackageFormat,
    jobs: &mut Vec<PackageJob>,
    ctx: &JobContext<'_>,
    images: &[String],
    arch_resolver: F,
) -> Result<()>
where
    F: Fn(Platform) -> Result<String>,
{
    let arch = arch_resolver(ctx.platform)?;
    for image in images {
        let subdir = sanitize_image_label(image);
        let dir = ctx.output_root.join(&subdir);
        fs::create_dir_all(&dir).with_context(|| {
            format!(
                "failed to prepare {} output directory {}",
                format.label().to_ascii_uppercase(),
                dir.display()
            )
        })?;
        jobs.push(PackageJob {
            format,
            image: image.clone(),
            platform: ctx.platform,
            installer_path: ctx.installer_path.to_path_buf(),
            script_path: ctx.script_path.to_path_buf(),
            output_dir: dir,
            arch: arch.clone(),
        });
    }
    Ok(())
}

fn ensure_linux_package_platform(platform: Platform) -> Result<()> {
    if platform == Platform::NoArch || !platform.as_str().starts_with("linux-") {
        bail!(
            "native package builds are only supported for linux platforms (received '{}')",
            platform.as_str()
        );
    }
    Ok(())
}

fn write_rpm_script(root: &Path) -> Result<PathBuf> {
    let path = root.join(RPM_SCRIPT_NAME);
    let script = format!(
        r#"#!/bin/bash
set -euo pipefail

ensure_rpmbuild() {{
    if command -v rpmbuild >/dev/null 2>&1; then
        return 0
    fi

    echo "Installing rpm-build tooling inside container..." >&2

    if command -v dnf >/dev/null 2>&1; then
        dnf -y install rpm-build tar gzip >/dev/null 2>&1 || return 1
    elif command -v yum >/dev/null 2>&1; then
        yum -y install rpm-build tar gzip >/dev/null 2>&1 || return 1
    elif command -v microdnf >/dev/null 2>&1; then
        microdnf install -y rpm-build tar gzip >/dev/null 2>&1 || return 1
    else
        return 1
    fi

    command -v rpmbuild >/dev/null 2>&1
}}

ensure_base64() {{
    if command -v base64 >/dev/null 2>&1; then
        return 0
    fi

    if command -v dnf >/dev/null 2>&1; then
        dnf -y install coreutils >/dev/null 2>&1 || return 1
    elif command -v yum >/dev/null 2>&1; then
        yum -y install coreutils >/dev/null 2>&1 || return 1
    elif command -v microdnf >/dev/null 2>&1; then
        microdnf install -y coreutils >/dev/null 2>&1 || return 1
    else
        return 1
    fi

    command -v base64 >/dev/null 2>&1
}}

if ! ensure_rpmbuild; then
    echo "rpmbuild command not found and automatic installation failed" >&2
    exit 1
fi

if ! ensure_base64; then
    echo "base64 command not found and automatic installation failed" >&2
    exit 1
fi

if [ -z "${{PKG_INSTALLER_PATH:-}}" ]; then
    echo "PKG_INSTALLER_PATH environment variable is required" >&2
    exit 1
fi

INSTALLER="$PKG_INSTALLER_PATH"
if [ ! -x "$INSTALLER" ]; then
    echo "installer not found or not executable at $INSTALLER" >&2
    exit 1
fi

SUMMARY=""
if [ -n "${{PKG_SUMMARY_B64:-}}" ]; then
    SUMMARY=$(printf '%s' "$PKG_SUMMARY_B64" | base64 -d)
fi
if [ -z "$SUMMARY" ]; then
    echo "package summary cannot be empty" >&2
    exit 1
fi
SUMMARY_SAFE=${{SUMMARY//%/%%}}

DESCRIPTION=""
if [ -n "${{PKG_DESCRIPTION_B64:-}}" ]; then
    DESCRIPTION=$(printf '%s' "$PKG_DESCRIPTION_B64" | base64 -d)
fi
DESCRIPTION_SAFE=${{DESCRIPTION//%/%%}}
if [ -z "$DESCRIPTION_SAFE" ]; then
    DESCRIPTION_SAFE="$SUMMARY_SAFE"
fi

WORKDIR="/tmp/conda-dist-package"
rm -rf "$WORKDIR"
mkdir -p "$WORKDIR"
ROOT="$WORKDIR/root"

PREFIX="$PKG_PREFIX"
if [[ "$PREFIX" != /* ]]; then
    echo "installation prefix must be absolute" >&2
    exit 1
fi

mkdir -p "$ROOT$PREFIX"
"$INSTALLER" "$ROOT$PREFIX"

tar -C "$ROOT" -czf "$WORKDIR/payload.tar.gz" .

TOPDIR="$WORKDIR/rpm"
mkdir -p "$TOPDIR"/{{BUILD,BUILDROOT,RPMS,SOURCES,SPECS,SRPMS}}
cp "$WORKDIR/payload.tar.gz" "$TOPDIR/SOURCES/payload.tar.gz"

SPEC="$TOPDIR/SPECS/package.spec"

cat > "$SPEC" <<__CONDADIST_SPEC__
Name: $PKG_NAME
Version: $PKG_VERSION
Release: %{{?conda_dist_release}}%{{!?conda_dist_release:1}}%{{?dist}}
Summary: $SUMMARY_SAFE
License: $PKG_LICENSE
Source0: payload.tar.gz
BuildArch: $PKG_RPM_ARCH
AutoReqProv: no

%description
$DESCRIPTION_SAFE

%prep
# nothing to do

%build
# nothing to do

%install
rm -rf %{{buildroot}}
mkdir -p %{{buildroot}}
tar -xzf %{{SOURCE0}} -C %{{buildroot}}

%files
%defattr(-,root,root,-)
$PKG_PREFIX
__CONDADIST_SPEC__

rpmbuild \
    --define "_topdir $TOPDIR" \
    --define "conda_dist_release ${{PKG_RELEASE:-1}}" \
    -bb "$SPEC"

RPM_SOURCE=$(find "$TOPDIR/RPMS" -type f -name "*.rpm" | head -n 1)
if [ ! -f "$RPM_SOURCE" ]; then
    echo "rpmbuild did not produce an rpm artifact" >&2
    exit 1
fi

mkdir -p "{OUTPUT_DEST_PATH}"
RPM_BASENAME=$(basename "$RPM_SOURCE")
cp "$RPM_SOURCE" "{OUTPUT_DEST_PATH}/$RPM_BASENAME"
"#
    );

    write_script(&path, &script)?;
    Ok(path)
}

fn write_deb_script(root: &Path) -> Result<PathBuf> {
    let path = root.join(DEB_SCRIPT_NAME);
    let script = format!(
        r#"#!/bin/bash
set -euo pipefail

APT_UPDATED=0

apt_update_once() {{
    if [ "$APT_UPDATED" -eq 0 ]; then
        apt-get update >/dev/null 2>&1 || return 1
        APT_UPDATED=1
    fi
}}

ensure_dpkg_deb() {{
    if command -v dpkg-deb >/dev/null 2>&1; then
        return 0
    fi

    if command -v apt-get >/dev/null 2>&1; then
        export DEBIAN_FRONTEND=noninteractive
        apt_update_once || return 1
        apt-get install -y dpkg-dev >/dev/null 2>&1 || return 1
    else
        return 1
    fi

    command -v dpkg-deb >/dev/null 2>&1
}}

ensure_base64() {{
    if command -v base64 >/dev/null 2>&1; then
        return 0
    fi

    if command -v apt-get >/dev/null 2>&1; then
        export DEBIAN_FRONTEND=noninteractive
        apt_update_once || return 1
        apt-get install -y coreutils >/dev/null 2>&1 || return 1
    else
        return 1
    fi

    command -v base64 >/dev/null 2>&1
}}

if ! ensure_dpkg_deb; then
    echo "dpkg-deb command not found and automatic installation failed" >&2
    exit 1
fi

if ! ensure_base64; then
    echo "base64 command not found and automatic installation failed" >&2
    exit 1
fi

if [ -z "${{PKG_INSTALLER_PATH:-}}" ]; then
    echo "PKG_INSTALLER_PATH environment variable is required" >&2
    exit 1
fi

INSTALLER="$PKG_INSTALLER_PATH"
if [ ! -x "$INSTALLER" ]; then
    echo "installer not found or not executable at $INSTALLER" >&2
    exit 1
fi

SUMMARY=""
if [ -n "${{PKG_SUMMARY_B64:-}}" ]; then
    SUMMARY=$(printf '%s' "$PKG_SUMMARY_B64" | base64 -d)
fi
if [ -z "$SUMMARY" ]; then
    echo "package summary cannot be empty" >&2
    exit 1
fi

DESCRIPTION=""
if [ -n "${{PKG_DESCRIPTION_B64:-}}" ]; then
    DESCRIPTION=$(printf '%s' "$PKG_DESCRIPTION_B64" | base64 -d)
fi

WORKDIR="/tmp/conda-dist-package"
rm -rf "$WORKDIR"
mkdir -p "$WORKDIR"
ROOT="$WORKDIR/root"

PREFIX="$PKG_PREFIX"
if [[ "$PREFIX" != /* ]]; then
    echo "installation prefix must be absolute" >&2
    exit 1
fi

mkdir -p "$ROOT$PREFIX"
"$INSTALLER" "$ROOT$PREFIX"

DEBIAN_DIR="$ROOT/DEBIAN"
mkdir -p "$DEBIAN_DIR"
CONTROL="$DEBIAN_DIR/control"

VERSION_FIELD="$PKG_VERSION"
if [ -n "${{PKG_RELEASE:-}}" ]; then
    VERSION_FIELD="$VERSION_FIELD-${{PKG_RELEASE}}"
fi

printf 'Package: %s\n' "$PKG_NAME" > "$CONTROL"
printf 'Version: %s\n' "$VERSION_FIELD" >> "$CONTROL"
printf 'Section: %s\n' "$PKG_SECTION" >> "$CONTROL"
printf 'Priority: %s\n' "$PKG_PRIORITY" >> "$CONTROL"
printf 'Architecture: %s\n' "$PKG_DEB_ARCH" >> "$CONTROL"
printf 'Maintainer: %s\n' "$PKG_AUTHOR" >> "$CONTROL"
printf 'Description: %s\n' "$SUMMARY" >> "$CONTROL"

if [ -n "$DESCRIPTION" ]; then
    while IFS= read -r line; do
        if [ -z "$line" ]; then
            printf ' .\n' >> "$CONTROL"
        else
            printf ' %s\n' "$line" >> "$CONTROL"
        fi
    done <<< "$DESCRIPTION"
else
    printf ' .\n' >> "$CONTROL"
fi

mkdir -p "{OUTPUT_DEST_PATH}"
dpkg-deb --build "$ROOT" "{OUTPUT_DEST_PATH}"
"#
    );

    write_script(&path, &script)?;
    Ok(path)
}

fn write_script(path: &Path, contents: &str) -> Result<()> {
    fs::write(path, contents)
        .with_context(|| format!("failed to write helper script {}", path.display()))?;

    #[cfg(unix)]
    {
        let mut perms = fs::metadata(path)
            .with_context(|| format!("failed to read metadata for {}", path.display()))?
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).with_context(|| {
            format!(
                "failed to mark helper script {} as executable",
                path.display()
            )
        })?;
    }

    #[cfg(not(unix))]
    {
        let mut perms = fs::metadata(path)
            .with_context(|| format!("failed to read metadata for {}", path.display()))?
            .permissions();
        perms.set_readonly(false);
        fs::set_permissions(path, perms).with_context(|| {
            format!(
                "failed to update permissions on helper script {}",
                path.display()
            )
        })?;
    }

    Ok(())
}
