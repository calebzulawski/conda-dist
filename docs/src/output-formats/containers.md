# Container Images

`conda-dist container <manifest>` builds OCI images from the manifest.

## Usage Example

```bash
conda-dist container examples/tar.toml
skopeo copy oci-archive:examples/portable-tar-compressed-container.oci.tar docker-daemon:portable-tar-compressed:1.0.0
docker run --rm portable-tar-compressed:1.0.0 tar --version
```

`conda-dist container` stages the resolved environment on the configured base
image and writes `<name>[-<subset>]-container.oci.tar`.
The example uses `skopeo` to load the archive into a Docker daemon; any OCI-aware
transport can be substituted.

## Characteristics

- **Output**: OCI archive suitable for use with Docker, Kubernetes, etc.
- **Base image**: Configurable (defaults to `gcr.io/distroless/base-debian12`).
- **Footprint**: Distroless base plus the packaged environment keeps layers minimal.
- **Multi-architecture**: Multiple platforms yield a single tag backed by per-platform images.
