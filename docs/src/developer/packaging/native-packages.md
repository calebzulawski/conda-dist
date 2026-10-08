# Native Packages (RPM/DEB)

Native packages are built in containers. The build uses a self-extracting installer to materialize the environment inside the container before packaging it.

## Build Behavior

- Installers are prepared for the requested platforms.
- A per-platform `package_plan.tsv` describes which native packages to build and where their payloads and metadata live in the packaging workspace.
- The packaging container receives the installer, the package plan, and an output directory via bind mounts.
- Inside the container, the packaging script installs the environment via the installer.
- Inside the container, the script runs `rpmbuild` or `dpkg-deb` and writes artifacts to the output directory.

## Bundle packages

With `package.bundle`:

- Each package contains its selected environment payload.
- No per-dependency subpackages are generated.
- The resulting RPM/DEB is self-contained.

## Modular packages

With `package.modular`:

- The base, component, and subset packages are metapackages with no payload.
- One dependency package is created per conda package, containing only that package's files.
- For Python noarch packages, the native package release is suffixed with the Python major/minor version (for example `py311`) so builds against different interpreters remain distinct.

To keep dependency packages tied to their metapackages, conda-dist encodes a dependency cycle:

- Base and component metapackages *depend on* their dependency packages with exact version/release pins and *provide* `lock-<package>` for each dependency. Subset metapackages depend on their component metapackages.
- Each dependency package *depends on* its corresponding `lock-<package>` virtual provide.

The metapackages act as the lockfile while allowing the environment to be split into native packages.

Example dependency cycle (environment name: `my-app`):

```
my-app (metapackage)
  Depends: my-app-conda-numpy (= 1.26.4-0), my-app-conda-python (= 3.12.1-0), ...
  Provides: lock-my-app-conda-numpy, lock-my-app-conda-python, ...

my-app-conda-numpy
  Depends: lock-my-app-conda-numpy

my-app-conda-python
  Depends: lock-my-app-conda-python
```
