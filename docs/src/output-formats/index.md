# Output Formats

Output formats consume the manifest and emit distributable artifacts. Each one
reuses the same dependency resolution step, then applies backend-specific
packaging.

- **Installers** — produce native self-extracting executables per target
  platform.
- **Container images** — stage the environment into a minimal base and output
  an OCI archive.

Subsequent sections describe the behaviour and outputs of each format.
