# OCI/Docker Images

Container images are built by running the self-extracting installer during the image build.

## Build Behavior

- Installers are prepared for the requested Linux platforms and staged in the build context.
- A multi-stage Dockerfile is generated. The first stage holds the installers; the final stage runs the matching installer to materialize the environment at the configured prefix.
- The image build is multi-platform; each platform run uses its corresponding installer.
- The resulting image sets `CONDA_PREFIX` and `PATH` to the configured prefix.
- The build output is exported as a multi-architecture OCI archive.
