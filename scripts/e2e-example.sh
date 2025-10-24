#!/usr/bin/env bash
set -euo pipefail

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
E2E_PREFIX_PARENT="$(mktemp -d)"
INSTALL_PREFIX="${E2E_PREFIX_PARENT}/bash-env"

echo "==> Building installer from examples/bash.toml"
RATTLER_CACHE_DIR="${E2E_CACHE_DIR}" \
    cargo run \
    --manifest-path "${REPO_ROOT}/conda-dist/Cargo.toml" \
    --bin conda-dist -- \
    installer \
    "${REPO_ROOT}/examples/bash.toml" \
    --output "${E2E_OUTPUT_DIR}"

INSTALLER_SCRIPT="$(find "${E2E_OUTPUT_DIR}" -maxdepth 1 -type f -name 'bash-*.sh' -print -quit)"
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
PACKAGE_JSON="$(bash "${INSTALLER_SCRIPT}" --list-packages-json)"
echo "$PACKAGE_JSON"

echo "==> Installing into ${INSTALL_PREFIX}"
mkdir -p "${INSTALL_PREFIX}"
bash "${INSTALLER_SCRIPT}" "${INSTALL_PREFIX}"

POST_INSTALL_LOG="${INSTALL_PREFIX}/post-install.log"
if [[ ! -s "${POST_INSTALL_LOG}" ]]; then
    echo "error: post-install log not created"
    exit 1
fi
echo "Post-install log created at ${POST_INSTALL_LOG}"
tail -n +1 "${POST_INSTALL_LOG}"

echo "==> Running installed bash"
"${INSTALL_PREFIX}/bin/bash" --version
"${INSTALL_PREFIX}/bin/bash" -c 'echo e2e-test-success'

echo "==> E2E flow completed successfully"
