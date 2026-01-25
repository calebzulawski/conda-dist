# Native Packages (RPM/DEB)

Native packages are built in containers. The build uses a self-extracting installer to materialize the environment inside the container before packaging it.

## Build Behavior

- Installers are prepared for the requested platforms.
- A per-platform `package_plan.tsv` describes which native packages to build and where their payloads and metadata live in the packaging workspace.
- The packaging container receives the installer, the package plan, and an output directory via bind mounts.
- Inside the container, the packaging script installs the environment via the installer.
- Inside the container, the script runs `rpmbuild` or `dpkg-deb` and writes artifacts to the output directory.

## `split_deps = false` (Single Package)

When `split_deps` is disabled:

- The base package contains the full environment payload.
- No per-dependency subpackages are generated.
- The resulting RPM/DEB is self-contained.

## `split_deps = true` (One Package per Conda Package)

When `split_deps` is enabled:

- The base package is a metapackage with no payload.
- One dependency package is created per conda package, containing only that package's files.
- For Python noarch packages, the native package release is suffixed with the Python major/minor version (for example `py311`) so builds against different interpreters remain distinct.

To ensure dependency packages are only installed alongside the base metapackage, conda-dist encodes a dependency cycle:

- The base package *depends on* each dependency package with exact version+build pins and *provides* `lock-<package>` for each dependency.
- Each dependency package *depends on* its corresponding `lock-<package>` virtual provide.

This makes the base package act as the lockfile while still allowing the environment to be split into native packages.

Example dependency cycle (environment name: `my-app`):

```
my-app (metapackage)
  Depends: my-app-numpy (= 1.26.4-0), my-app-python (= 3.12.1-0), ...
  Provides: lock-my-app-numpy, lock-my-app-python, ...

my-app-numpy
  Depends: lock-my-app-numpy

my-app-python
  Depends: lock-my-app-python
```
