# Native Packages

`conda-dist package <manifest>` builds RPM and DEB archives for Linux targets.

## Usage Example

```toml
[package.images.rocky]
type = "rpm"
image = "rockylinux:9"

[package.images.ubuntu]
type = "deb"
image = "ubuntu:24.04"
```

```bash
conda-dist package app.toml
```

Packages are written beneath `<output-dir>/<image-name>/`, grouped by the image
name from the manifest. The output directory defaults to the manifest directory,
and each image directory contains the generated RPM/DEB artifacts.

Use `--image <name>` to select a subset of images from the manifest.

## Characteristics

- **Output**: RPM/DEB archives organized per container image.
- **Platforms**: Supports all Linux targets listed in the manifest.
- **Images**: Works with docker/podman-compatible distribution images.
- **Runtime dependencies**: None required on the target host.
