#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/.." && pwd)"

platform_from_triple() {
    case "$1" in
        x86_64-unknown-linux-gnu|x86_64-unknown-linux-musl) echo "linux-64" ;;
        aarch64-unknown-linux-gnu|aarch64-unknown-linux-musl) echo "linux-aarch64" ;;
        x86_64-apple-darwin) echo "osx-64" ;;
        aarch64-apple-darwin) echo "osx-arm64" ;;
        x86_64-pc-windows-msvc) echo "win-64" ;;
        *)
            echo "Unsupported host triple: $1" >&2
            exit 1
            ;;
    esac
}

host_triple="$(rustc -vV | awk '/^host: / { print $2 }')"
conda_platform="$(platform_from_triple "${host_triple}")"

build_cmd=()
artifact=""

case "${conda_platform}" in
    linux-64)
        build_cmd=(cross build --manifest-path "${repo_root}/Cargo.toml" -p conda-dist-install --release --target x86_64-unknown-linux-musl)
        artifact="${repo_root}/target/x86_64-unknown-linux-musl/release/conda-dist-install"
        ;;
    linux-aarch64)
        build_cmd=(cross build --manifest-path "${repo_root}/Cargo.toml" -p conda-dist-install --release --target aarch64-unknown-linux-musl)
        artifact="${repo_root}/target/aarch64-unknown-linux-musl/release/conda-dist-install"
        ;;
    *)
        build_cmd=(cargo build --manifest-path "${repo_root}/Cargo.toml" -p conda-dist-install --release)
        artifact="${repo_root}/target/release/conda-dist-install"
        ;;
esac

echo "Building conda-dist-install for host ${conda_platform}"
"${build_cmd[@]}"

if [[ ! -f "${artifact}" ]]; then
    echo "Expected artifact not found: ${artifact}" >&2
    exit 1
fi

installers_dir="${repo_root}/conda-dist/installers"
mkdir -p "${installers_dir}"

target_path="${installers_dir}/${conda_platform}"
cp "${artifact}" "${target_path}"
echo "Copied ${artifact} -> ${target_path}"
