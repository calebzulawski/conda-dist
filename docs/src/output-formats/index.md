# Output Formats

Output formats consume the manifest and emit distributable artifacts. Each one
reuses the same dependency resolution step, then applies backend-specific
packaging.

- **Installers** — produce native self-extracting executables per target
  platform.
- **Container images** — stage the environment into a minimal base and output
  an OCI archive.
- **Native packages** — produce RPM and DEB artifacts for Linux distributions.

Artifacts default to the manifest directory. Use `--output-dir <directory>` with
installer or native package builds to write them elsewhere.
