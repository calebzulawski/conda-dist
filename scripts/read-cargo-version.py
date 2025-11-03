#!/usr/bin/env python3
"""Emit release version metadata derived from conda-dist/Cargo.toml."""

from __future__ import annotations

import argparse
import sys


def read_version(path: str) -> tuple[str, str]:
    import tomllib

    with open(path, "rb") as fp:
        data = tomllib.load(fp)

    raw_version = data.get("package", {}).get("version", "").strip()
    if not raw_version:
        raise SystemExit(f"Version not found in {path}")

    if raw_version.startswith("v"):
        tag = raw_version
        version = raw_version[1:]
    else:
        version = raw_version
        tag = f"v{raw_version}"

    return version, tag


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "cargo_toml",
        nargs="?",
        default="conda-dist/Cargo.toml",
        help="Path to the package Cargo.toml (default: conda-dist/Cargo.toml)",
    )
    args = parser.parse_args()

    version, tag = read_version(args.cargo_toml)
    print(f"version={version}")
    print(f"tag={tag}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
