#!/usr/bin/env bash
set -euo pipefail

MANIFEST=${1:-examples/bash.toml}
TEST_COMMAND=${2:-bin/bash}

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
pushd "$REPO_ROOT" >/dev/null

cleanup() {
    if [[ "${KEEP_E2E_ARTIFACTS:-}" != "1" ]]; then
        [[ -n "${E2E_CACHE_DIR:-}" && -d "${E2E_CACHE_DIR:-}" ]] && rm -rf "${E2E_CACHE_DIR}"
        [[ -n "${E2E_OUTPUT_DIR:-}" && -d "${E2E_OUTPUT_DIR:-}" ]] && rm -rf "${E2E_OUTPUT_DIR}"
        [[ -n "${E2E_PREFIX_PARENT:-}" && -d "${E2E_PREFIX_PARENT:-}" ]] && rm -rf "${E2E_PREFIX_PARENT}"
    fi
    popd >/dev/null || true
}
trap cleanup EXIT

echo "==> Bootstrapping bundled installer"
"${REPO_ROOT}/scripts/bootstrap-dev.sh"

E2E_CACHE_DIR="$(mktemp -d)"
E2E_OUTPUT_DIR="$(mktemp -d)"
E2E_INSTALL_DIR="$(mktemp -d)"

echo "==> Building installer from ${MANIFEST}"
RATTLER_CACHE_DIR="${E2E_CACHE_DIR}" \
    cargo run \
    --bin conda-dist -- \
    installer \
    "${MANIFEST}" \
    --installer-platform host \
    --output "${E2E_OUTPUT_DIR}"

INSTALLER_SCRIPT="$(find "${E2E_OUTPUT_DIR}" -maxdepth 1 -type f -name '*.sh' -print -quit)"
if [[ -z "${INSTALLER_SCRIPT}" ]]; then
    echo "error: no installer script produced"
    exit 1
fi
echo "==> Installer generated at ${INSTALLER_SCRIPT}"

echo "==> Displaying bundle summary"
bash "${INSTALLER_SCRIPT}" --summary

echo "==> Listing packages (table)"
bash "${INSTALLER_SCRIPT}" --list-packages

echo "==> Listing packages (JSON)"
bash "${INSTALLER_SCRIPT}" --list-packages-json

echo "==> Installing into ${E2E_INSTALL_DIR}"
mkdir -p "${E2E_INSTALL_DIR}"
bash "${INSTALLER_SCRIPT}" "${E2E_INSTALL_DIR}"

echo "==> Running installed bash"
"${E2E_INSTALL_DIR}/${TEST_COMMAND}" --version

echo "==> E2E flow completed successfully"
