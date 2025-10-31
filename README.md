# conda-dist 🐍 ⇢ 📦

conda-dist packages conda applications, producing portable installers and Docker images for use outside conda environments.

## Overview

conda-dist lets you bundle an application and its dependencies into a self-contained package.
Use it to:
* Distribute apps portably (similar to snap or AppImage)
* Build reproducible Docker images for CI/CD
* Simplify application deployments

## Example

To package bash, create bash.toml:

```toml
name = "bash"
version = "1.0.0"
author = "Example Maintainers"
channels = ["conda-forge"]
platforms = ["linux-64"]

[dependencies]
bash = "*"
```

### Installer

To create a portable installer, run:

```bash
conda-dist installer bash.toml
```

To install the bash application:

Invoke the generated `bash-linux-64` executable and point it at an install
directory:

```bash
./bash-linux-64 <install dir>
<install dir>/bin/bash --version
```

### Docker Image

To create a docker image, run:

```bash
conda-dist container bash.toml
```

The generated image contains just the bash application:

```bash
docker run bash:1.0.0 bash -c "echo hello"
```

## License

conda-dist is licensed under the Apache License, Version 2.0.
