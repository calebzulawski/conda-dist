# Installers

`conda-dist installer <manifest>` produces `<name>-<platform>` self-extracting
installers, plus one `<name>-<subset>-<platform>` installer per subset.

## Usage Example

```bash
conda-dist installer examples/tar.toml
./examples/portable-tar-linux-64 ./tar-env
./tar-env/bin/tar --version
```

The command caches downloads and writes installers to the manifest directory.
Each installer unpacks the bundled environment into the installation path you
provide, with no external runtime requirements.

## Characteristics

- **Output**: Native executable archive, one per target platform and subset.
- **Installation prefix**: User-supplied path, installable anywhere.
- **Runtime dependencies**: None required on the target host.
- **Reproducibility**: Bundles the locked package set used at build time.
- **Transport**: Compressed payload suitable for artifact stores or offline delivery.
