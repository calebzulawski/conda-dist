# Configuration Reference

`conda-dist` reads a single TOML manifest. Every command consumes the same
configuration, so you describe the environment once and reuse it for installers,
containers, or other outputs. A minimal manifest looks like:

```toml
name = "myapp"
version = "1.2.0"
author = "John Doe"
license = "Proprietary"
channels = ["conda-forge"]
platforms = ["linux-64", "osx-arm64"]

[dependencies]
python = "3.11.*"
pandas = "^2.2"
```

The remaining sections document each supported key, including optional tables
for format-specific settings.
