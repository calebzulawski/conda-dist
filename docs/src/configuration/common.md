# Common Settings

These keys describe the application and apply to every output format.

## Manifest fields

- `name` (required) — Application name used in installer filenames, archives,
  native packages, and container tags.
- `version` (required) — Application version used in container tags and native
  package metadata.
- `author` (required) — Maintainer identifier bundled into metadata.
- `license` (optional) — Application license for native packages. Defaults to
  `"Proprietary"`.
- `channels` (required) — Array of Conda channels, in priority order.
- `platforms` (required) — Array of target Conda platforms, such as `"linux-64"`
  and `"osx-arm64"`.

## Dependencies

- `[dependencies]` (required) — Base package requirements included in every
  artifact. Values use Conda match-spec syntax; `"*"` accepts any version.

```toml
[dependencies]
python = "3.12.*"
tar = "*"
```

## Components and subsets

Components add optional packages to the base. All components are solved
together, keeping shared package versions consistent.

- `components.<name>.dependencies` — Additional Conda package requirements.
  Defaults to an empty table.
- `components.<name>.includes` — Other components to include recursively.
  Defaults to `[]`; a component can group others without adding packages.
- `subsets.<name>.components` — Components to publish together as a named
  artifact, including the base. Defaults to `[]` for a base-only subset.

```toml
[components.gzip.dependencies]
gzip = "*"

[components.zstd.dependencies]
zstd = "*"

[components.compression]
includes = ["gzip", "zstd"]

[subsets.compressed]
components = ["compression"]
```

Installers bundle all components by default; `--components` selects what to
install. Each subset also produces a `<name>-<subset>-<platform>` installer.
Containers and native packages produce a base-only artifact and a
`<name>-<subset>` artifact for each subset.

## Metadata

All fields in `[metadata]` are optional:

- `summary` — Short application description. Defaults to `name`.
- `description` — Longer description for installer summaries and native packages.
- `release_notes` — Release notes for installer summaries and native packages.
- `featured_packages` — Conda packages highlighted in installer summaries.
  Defaults to `[]`.

```toml
[metadata]
summary = "Portable Tar"
description = "GNU tar with optional compression tools."
release_notes = "Initial release."
featured_packages = ["tar"]
```

## Virtual packages

Virtual packages describe target capabilities to the solver using fixed defaults
rather than detecting the build host. Override them in
`[virtual_packages.default]` or `[virtual_packages.<platform>]`.

- `linux` — Linux kernel version. Defaults to `"5.4"` on Linux.
- `osx` — macOS version. Defaults to `"13.0"` on macOS.
- `win` — Windows version. Defaults to `"10"` on Windows; `""` leaves it unversioned.
- `libc.family` — libc family. Defaults to `"glibc"`.
- `libc.version` — Required when setting `libc`. Defaults to `"2.17"` on Linux
  when no override is supplied.
- `cuda` — CUDA version. Absent by default; `""` removes it.

A platform-specific table replaces the `default` table. Omitted fields retain
their built-in defaults.

```toml
[virtual_packages.default]
cuda = "12.2"

[virtual_packages.linux-64]
linux = "5.15"
libc = { family = "glibc", version = "2.31" }
cuda = "12.2"
```
