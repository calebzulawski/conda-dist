# Common Settings

These keys apply to every build target and appear at the top level of the
manifest.

## Required fields

- `name` — ASCII string composed of letters, digits, `-`, `_`, or `.`. Used when
  naming installers, archives, and tags.
- `version` — ASCII string without whitespace. Appended to artifact names and
  container tags.
- `author` — Free-form maintainer identifier bundled into metadata.
- `license` — License identifier embedded in native package metadata. Defaults
  to `Proprietary` if omitted.
- `channels` — Non-empty array of Conda channels evaluated in order of
  precedence.
- `platforms` — Non-empty array of Conda platforms (for example `linux-64`,
  `osx-arm64`). Each platform yields a distinct installer executable.

## Dependencies

Declare package requirements with a table of Conda match specs:

```toml
[dependencies]
tar = "*"
```

## Components and subsets

Components add optional packages to `[dependencies]`. All components are solved
together, keeping shared package versions consistent.

```toml
[components.gzip.dependencies]
gzip = "*"

[components.zstd.dependencies]
zstd = "*"

[subsets.compressed]
components = ["gzip", "zstd"]
```

Here, the base provides GNU tar, and `compressed` adds gzip and Zstandard
support. Components remain individually selectable; subsets define the
contents of each artifact. Components can include other components with
`includes = ["other-component"]`.

Installers include all components by default and also produce a
`<name>-<subset>-<platform>` installer containing each subset's components.
Each installer installs its whole collection unless a component selection is
provided. Container and bundled native builds produce the base artifact plus one
`<name>-<subset>` artifact per subset. With `package.modular`, components and
subsets become separately installable metapackages.

## Metadata

Populate optional descriptive fields for installers and summary output:

```toml
[metadata]
summary = "MyApp command suite"
description = "Command-line utilities for data preparation."
release_notes = "- Initial publication."
featured_packages = ["tar"]
```

## Virtual packages

`conda-dist` seeds each platform with sensible defaults for synthetic virtual
packages instead of probing the build host. Override values only when you need
 to pin a specific runtime characteristic.

```toml
[virtual_packages.default]
linux = "5.15"
libc = { family = "glibc", version = "2.31" }

[virtual_packages.linux-64]
cuda = "12.2"
```

Use the `default` table for cross-platform values and add per-platform tables to
override individual targets. Supported keys are `linux`, `osx`, `win`, `libc`,
and `cuda`.

## Package settings

Configure native RPM/DEB packaging:

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

`package.bundle` produces complete base and subset environments.
`package.modular` produces metapackages and shared dependency packages.
The optional `release` field controls the RPM/DEB release suffix (defaults to `1`).
Each mode defines build images by name. Outputs go under `bundle/<image-name>/`
or `modular/<image-name>/`. Both modes may be configured together.
