# Container Images

`conda-dist container <manifest>` builds an OCI image from the manifest.

## Usage Example

```bash
conda-dist container app.toml
skopeo copy oci-archive:app-container.oci.tar docker-daemon://app:1.0.0
docker run app:1.0.0 env/bin/python --version
```

`conda-dist container` stages the resolved prefix on the configured base image
and writes an OCI archive (default `<name>-container.oci.tar` alongside the
manifest). The example uses `skopeo` to load the archive into a Docker daemon;
any OCI-aware transport can be substituted. Builds run with rootless OCI
tooling, so no Docker daemon privileges are required.

## Characteristics

- **Output**: OCI archive suitable for use with Docker, Kubernetes, etc.
- **Base image**: Configurable (defaults to `gcr.io/distroless/base-debian12`).
- **Footprint**: Distroless base plus the packaged environment keeps layers minimal.
- **Multi-architecture**: Multiple platforms yield a single tag backed by per-platform images.
