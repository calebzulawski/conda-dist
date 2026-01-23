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
python = "3.11.*"
pandas = "^2.2"
```

## Metadata

Populate optional descriptive fields for installers and summary output:

```toml
[metadata]
summary = "MyApp command suite"
description = "Command-line utilities for data preparation."
release_notes = "- Initial publication."
featured_packages = ["python", "pandas"]
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
split_deps = false
release = "1"
```

When enabled, `split_deps` emits a metapackage plus individual native packages
for each transitive dependency.
The optional `release` field controls the RPM/DEB release suffix applied to the
base package (defaults to `1`).
