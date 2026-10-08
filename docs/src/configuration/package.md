# Package Settings

`[package]` configures native RPM/DEB packaging:

- `release` — Application package release suffix. Defaults to `"1"`.
- `bundle.images` — Named build images for complete base and subset packages.
  Defaults to an empty table.
- `modular.images` — Named build images for base, component, and subset
  metapackages with shared dependency packages. Defaults to an empty table.

Each build-image entry has two required fields:

- `type` — Package format, `"rpm"` or `"deb"`.
- `image` — Container image used to build the packages.

```toml
[package]
release = "1"

[package.bundle.images.rocky]
type = "rpm"
image = "rockylinux:9"

[package.modular.images.ubuntu]
type = "deb"
image = "ubuntu:24.04"
```

Both modes may be configured together. Outputs go under `bundle/<image-name>/`
or `modular/<image-name>/`. The installation prefix is set by `container.prefix`,
defaulting to `/opt/<name>`.
