# conda-dist 🐍 ⇢ 📦

conda-dist packages conda applications for use outside conda environments by producing portable installers, system packages, and Docker images.

## Overview

conda-dist lets you bundle an application and its dependencies into a self-contained package.

It can produce:
* Self-extracting installers
* Container images (OCI format compatible with Docker, Podman, K8s etc.)
* Native system packages (RPM/DEB)

Use it to:
* Distribute apps portably (similar to snap or AppImage)
* Build reproducible Docker images for CI/CD
* Simplify application deployments

For a complete reference, see the [manual](https://calebzulawski.github.io/conda-dist).

## Example

To package GNU tar, create `tar.toml`:

```toml
name = "portable-tar"
version = "1.0.0"
author = "Example Maintainers"
channels = ["conda-forge"]
platforms = ["linux-64"]

[dependencies]
tar = "*"
```

### Installer

To create a portable installer, run:

```bash
conda-dist installer tar.toml
```

Install the environment:

```bash
./portable-tar-linux-64 ./tar-env
./tar-env/bin/tar --version
```

### Docker Image

To create a docker image, run:

```bash
conda-dist container tar.toml
skopeo copy oci-archive:portable-tar-container.oci.tar docker-daemon:portable-tar:1.0.0
docker run --rm portable-tar:1.0.0 tar --version
```

### Native Packages

Define package images in the manifest:

```toml
[package.bundle.images.redhat]
type = "rpm"
image = "redhat/ubi10"

[package.bundle.images.ubuntu]
type = "deb"
image = "ubuntu:24.04"
```

Then create native RPM and DEB artifacts:

```bash
conda-dist package tar.toml
```

### Lockfile Maintenance

To refresh the manifest lockfile without producing artifacts, run:

```bash
conda-dist lock tar.toml
```

Use the global `--locked` flag to assert the lockfile is current:

```bash
conda-dist --locked lock tar.toml
```

## License

conda-dist is licensed under the Apache License, Version 2.0.
