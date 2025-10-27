#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/.." && pwd)"

if [[ $OSTYPE == 'darwin'* ]]; then
    declare -a builds=(
        "osx-64:x86_64-apple-darwin"
        "osx-arm64:aarch64-apple-darwin"
    )
    cargo=cargo
else
    declare -a builds=(
        "linux-64:x86_64-unknown-linux-musl"
        "linux-aarch64:aarch64-unknown-linux-musl"
        "linux-armv7l:armv7-unknown-linux-musleabihf"
        # "linux-ppc64le:powerpc64le-unknown-linux-musl"
    )
    cargo=cross
fi

installers_dir="${repo_root}/conda-dist/installers"
mkdir -p "${installers_dir}"
rm -f "${installers_dir}/"*

for entry in "${builds[@]}"; do
    IFS=":" read -r platform target <<<"${entry}"
    echo "Building ${platform} (${target}) with cross"
    $cargo build --manifest-path "${repo_root}/Cargo.toml" -p conda-dist-install --release --target "${target}"

    artifact="${repo_root}/target/${target}/release/conda-dist-install"
    if [[ ! -f "${artifact}" ]]; then
        echo "Expected artifact not found: ${artifact}" >&2
        exit 1
    fi

    cp "${artifact}" "${installers_dir}/${platform}"
    echo "Copied installer for ${platform} -> ${installers_dir}/${platform}"
done
