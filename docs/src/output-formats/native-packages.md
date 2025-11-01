# Native Packages

`conda-dist package <manifest>` builds RPM and DEB archives for Linux targets.

## Usage Example

```bash
conda-dist package app.toml \
  --rpm-image rockylinux:9 \
  --deb-image ubuntu:24.04
```

Packages are written beneath
`<manifest-dir>/<name>-packages/<format>/<platform>/<image>/`, grouped by the
target platform and container image used.

## Characteristics

- **Output**: RPM/DEB archives organised per platform and image.
- **Platforms**: Supports all Linux targets listed in the manifest.
- **Images**: Works with docker/podman-compatible distribution images.
- **Runtime dependencies**: None required on the target host.
