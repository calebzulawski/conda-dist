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

echo "Building conda-dist-install for host (${host_triple}) -> ${conda_platform}"
cargo build --manifest-path "${repo_root}/Cargo.toml" -p conda-dist-install --release

installers_dir="${repo_root}/conda-dist/installers"
mkdir -p "${installers_dir}"

source_path="${repo_root}/target/release/conda-dist-install"
target_path="${installers_dir}/${conda_platform}"

cp "${source_path}" "${target_path}"
echo "Copied ${source_path} -> ${target_path}"
