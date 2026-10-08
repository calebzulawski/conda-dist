# Native Packages

`conda-dist package <manifest>` builds RPM and DEB archives for Linux targets.

## Usage Example

```toml
[package.bundle.images.redhat]
type = "rpm"
image = "redhat/ubi10"

[package.bundle.images.ubuntu]
type = "deb"
image = "ubuntu:24.04"
```

```bash
conda-dist package examples/tar.toml
```

Packages are written beneath `<output-dir>/<mode>/<image-name>/`, where mode is
`bundle` or `modular`. The output directory defaults to the manifest directory.

Use `--image <name>` to select a subset of images from the manifest.

## Characteristics

- **Output**: RPM/DEB archives organized per container image.
- **Platforms**: Supports all Linux targets listed in the manifest.
- **Images**: Works with docker/podman-compatible distribution images.
- **Runtime dependencies**: None required on the target host.
