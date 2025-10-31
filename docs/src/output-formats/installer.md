# Installers

`conda-dist installer <manifest>` produces a native self-extracting installer
for each target platform.

## Usage Example

```bash
conda-dist installer app.toml
./app-linux-64 /opt/app
/opt/app/bin/python --version
```

The command caches downloads and writes one native installer executable per
platform (defaulting to `<name>-<platform>` in the manifest directory). Each
installer unpacks the bundled prefix into the installation path you provide, with
no external runtime requirements.

## Characteristics

- **Output**: Native executable archive, one per target platform.
- **Installation prefix**: User-supplied path; contents are relocatable.
- **Runtime dependencies**: None required on the target host.
- **Reproducibility**: Bundles the locked package set used at build time.
- **Transport**: Compressed payload suitable for artifact stores or offline delivery.
