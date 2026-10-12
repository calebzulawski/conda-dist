# Configuration Reference

`conda-dist` reads a single TOML manifest. Every command consumes the same
configuration, so you describe the environment once and reuse it for installers,
containers, or other outputs. A minimal manifest looks like:

```toml
name = "portable-tar"
version = "1.0.0"
author = "Example Maintainers"
channels = ["conda-forge"]
platforms = ["linux-64", "osx-arm64"]

[dependencies]
tar = "*"
```

The remaining sections document each supported key, including optional tables
for format-specific settings.
