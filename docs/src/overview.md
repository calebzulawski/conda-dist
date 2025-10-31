# Overview

`conda-dist` is a command-line tool that converts a Conda environment manifest
into deliverables. It reads a single `conda-dist.toml` file, resolves the
requested packages, and prepares the build artifacts.

Available output families include:

- Native installers that unpack the environment into a user-specified prefix.
- OCI container images that embed the environment on top of a minimal base.
- Native packages (RPM/DEB) built for Linux targets.

The remaining chapters document the manifest format and describe how each
backend consumes it.
