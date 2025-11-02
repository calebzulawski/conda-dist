# Native Packages

`conda-dist package <manifest>` builds RPM and DEB archives for Linux targets.

## Usage Example

```bash
conda-dist package app.toml \
  --rpm-image rockylinux:9 \
  --deb-image ubuntu:24.04
```

Packages are written beneath `<output-dir>/<image>/`, grouped by the container
image used. The output directory defaults to the manifest directory, and each
image directory contains the generated RPM/DEB artifacts.

## Characteristics

- **Output**: RPM/DEB archives organized per container image.
- **Platforms**: Supports all Linux targets listed in the manifest.
- **Images**: Works with docker/podman-compatible distribution images.
- **Runtime dependencies**: None required on the target host.
