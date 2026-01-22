#!/bin/bash
set -euo pipefail

ensure_rpmbuild() {
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
}

maybe_chown() {
    if [ -n "${PKG_UID:-}" ] && [ -n "${PKG_GID:-}" ] && command -v chown >/dev/null 2>&1; then
        chown -R "$PKG_UID:$PKG_GID" "$1" >/dev/null 2>&1 || true
    fi
}

if ! ensure_rpmbuild; then
    echo "rpmbuild command not found and automatic installation failed" >&2
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
while IFS=$'\t' read -r name payload_mode spec_rel topdir_rel release filelist_rel; do
    if [ -z "$name" ]; then
        continue
    fi
    SPEC="$PKG_PACKAGING_ROOT/$spec_rel"
    TOPDIR="$PKG_PACKAGING_ROOT/$topdir_rel"
    mkdir -p "$TOPDIR/BUILD" "$TOPDIR/BUILDROOT" "$TOPDIR/RPMS" "$TOPDIR/SOURCES" "$TOPDIR/SPECS" "$TOPDIR/SRPMS"
    if [ ! -f "$SPEC" ]; then
        echo "spec file not found at $SPEC" >&2
        exit 1
    fi

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
        payload_root="$TOPDIR/SOURCES/payload-root"
        rm -rf "$payload_root"
        mkdir -p "$payload_root"
        if [ "$payload_mode" = "files" ] && [ "$filelist_rel" != "-" ]; then
            while IFS= read -r relpath; do
                [ -n "$relpath" ] || continue
                mkdir -p "$payload_root/$(dirname "$relpath")"
                cp -a "/$relpath" "$payload_root/$relpath"
            done < "$PKG_PACKAGING_ROOT/$filelist_rel"
        else
            mkdir -p "$payload_root$PKG_PREFIX"
            cp -a "$PKG_PREFIX"/. "$payload_root$PKG_PREFIX"/
        fi
    fi

    rpmbuild \
        --define "_topdir $TOPDIR" \
        --define "conda_dist_release ${release:-1}" \
        -bb "$SPEC"

    RPM_SOURCE=$(find "$TOPDIR/RPMS" -type f -name "${name}-*.rpm" | head -n 1)
    if [ ! -f "$RPM_SOURCE" ]; then
        echo "rpmbuild did not produce an rpm artifact for $name" >&2
        exit 1
    fi

    mkdir -p "{OUTPUT_DEST_PATH}"
    RPM_BASENAME=$(basename "$RPM_SOURCE")
    cp "$RPM_SOURCE" "{OUTPUT_DEST_PATH}/$RPM_BASENAME"
    maybe_chown "$TOPDIR"
done < "$PKG_PACKAGING_ROOT/$PKG_PACKAGE_PLAN"

maybe_chown "{OUTPUT_DEST_PATH}"
