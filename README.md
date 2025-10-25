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
channels = ["conda-forge"]
platforms = ["linux-64"]

[dependencies]
bash = "*"
```

Then run:

```bash
conda-dist installer bash.toml
```

This generates an installer you can run anywhere:

```bash
bash-linux-64.sh <install dir>
<install dir>/bin/bash --version
```

## License

conda-dist is licensed under the Apache License, Version 2.0.
