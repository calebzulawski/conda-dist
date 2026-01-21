#!/bin/bash
set -euo pipefail

APT_UPDATED=0

apt_update_once() {
    if [ "$APT_UPDATED" -eq 0 ]; then
        apt-get update >/dev/null 2>&1 || return 1
        APT_UPDATED=1
    fi
}

ensure_dpkg_deb() {
    if command -v dpkg-deb >/dev/null 2>&1; then
        return 0
    fi

    if command -v apt-get >/dev/null 2>&1; then
        export DEBIAN_FRONTEND=noninteractive
        apt_update_once || return 1
        apt-get install -y dpkg-dev tar >/dev/null 2>&1 || return 1
    else
        return 1
    fi

    command -v dpkg-deb >/dev/null 2>&1
}

maybe_chown() {
    if [ -n "${PKG_UID:-}" ] && [ -n "${PKG_GID:-}" ] && command -v chown >/dev/null 2>&1; then
        chown -R "$PKG_UID:$PKG_GID" "$1" >/dev/null 2>&1 || true
    fi
}

if ! ensure_dpkg_deb; then
    echo "dpkg-deb command not found and automatic installation failed" >&2
    exit 1
fi

if [ -z "${PKG_PACKAGING_ROOT:-}" ]; then
    echo "PKG_PACKAGING_ROOT environment variable is required" >&2
    exit 1
fi

if [ -z "${PKG_PACKAGE_PLAN:-}" ]; then
    echo "PKG_PACKAGE_PLAN environment variable is required" >&2
    exit 1
fi

if [ ! -f "$PKG_PACKAGING_ROOT/$PKG_PACKAGE_PLAN" ]; then
    echo "package plan not found at $PKG_PACKAGING_ROOT/$PKG_PACKAGE_PLAN" >&2
    exit 1
fi

installed=0
while IFS=$'\t' read -r name payload_mode control_rel root_rel _ filelist_rel; do
    if [ -z "$name" ]; then
        continue
    fi
    CONTROL="$PKG_PACKAGING_ROOT/$control_rel"
    ROOT="$PKG_PACKAGING_ROOT/$root_rel"
    rm -rf "$ROOT"
    mkdir -p "$ROOT/DEBIAN"
    if [ ! -f "$CONTROL" ]; then
        echo "control file not found at $CONTROL" >&2
        exit 1
    fi
    cp "$CONTROL" "$ROOT/DEBIAN/control"

    if [ "$payload_mode" != "none" ]; then
        if [ -z "${PKG_INSTALLER:-}" ]; then
            echo "PKG_INSTALLER environment variable is required for payload packages" >&2
            exit 1
        fi
        if [ -z "${PKG_PREFIX:-}" ]; then
            echo "PKG_PREFIX environment variable is required when PKG_INSTALLER is set" >&2
            exit 1
        fi
        if [ "$installed" -eq 0 ]; then
            "$PKG_INSTALLER" "$PKG_PREFIX"
            installed=1
        fi
        if [ "$payload_mode" = "files" ] && [ "$filelist_rel" != "-" ]; then
            tar -C / -cf - --files-from "$PKG_PACKAGING_ROOT/$filelist_rel" | tar -C "$ROOT" -xf -
        else
            tar -C / -cf - "${PKG_PREFIX#/}" | tar -C "$ROOT" -xf -
        fi
    fi

    mkdir -p "{OUTPUT_DEST_PATH}"
    dpkg-deb --build "$ROOT" "{OUTPUT_DEST_PATH}"
    maybe_chown "$ROOT"
done < "$PKG_PACKAGING_ROOT/$PKG_PACKAGE_PLAN"

maybe_chown "{OUTPUT_DEST_PATH}"
