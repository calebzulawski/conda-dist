# Self-Extracting Installer

## Inputs

The installer output combines two inputs:

- A platform-specific installer binary embedded in the `conda-dist` build (`conda-dist/installers/*` compiled into the binary via `build.rs`).
- A tar.gz payload containing the staged channel.

## Payload Layout

The payload is a gzipped tar archive with a single root directory named after the environment. The archive includes:

- `conda-lock.yml` (the lockfile copied into the channel directory).
- The channel subdirectories: `noarch/` and the target platform subdir (for example `linux-64/`).
- `repodata.json` and any other channel index artifacts in the root of each subdir.

## Self-Extracting Binary Layout

The final installer file is created by appending metadata and payload data to the installer stub. The layout is:

```
[installer bytes]
[bundle metadata JSON]
[u64 metadata length, little-endian]
[tar.gz payload bytes]
[u64 payload length, little-endian]
"CONDADIST!"
```

The embedded metadata JSON matches the bundle metadata from the manifest. The trailing length fields and magic marker allow the installer to find the payload and metadata by reading backward from the end of the executable.

## Runtime Behavior

At install time the installer:

- Reads the embedded metadata and payload length footer from its own executable.
- Extracts the tar.gz payload to a temporary directory.
- Loads the lockfile from the extracted channel directory.
- Uses the extracted channel (a file:// URL) to install the locked environment into the requested prefix.
