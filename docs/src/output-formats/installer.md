# Installers

`conda-dist installer <manifest>` produces `<name>-<platform>` self-extracting
installers, plus one `<name>-<subset>-<platform>` installer per subset.

## Usage Example

```bash
conda-dist installer examples/tar.toml
./examples/portable-tar-linux-64 ./tar-env
./tar-env/bin/tar --version
```

Each installer unpacks the bundled environment into the installation path you
provide, with no external runtime requirements.

Installers include all their components by default. Select individual
components with `--components`, use `none` for the base alone, or run the subset
installer:

```bash
./examples/portable-tar-linux-64 ./tar-gzip --components gzip
./examples/portable-tar-linux-64 ./tar-base --components none
./examples/portable-tar-compressed-linux-64 ./tar-compressed
```

## Characteristics

- **Output**: Native executable archive, one per target platform and subset.
- **Installation prefix**: User-supplied path, installable anywhere.
- **Runtime dependencies**: None required on the target host.
- **Reproducibility**: Bundles the locked package set used at build time.
- **Transport**: Compressed payload suitable for artifact stores or offline delivery.
