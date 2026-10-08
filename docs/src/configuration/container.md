# Container Settings

`[container]` configures OCI image output. All fields are optional:

- `base_image` — Base image for the application. Defaults to
  `"gcr.io/distroless/base-debian12"`.
- `prefix` — Absolute installation path in containers and native packages.
  Defaults to `/opt/<name>` and is shared by the base and subsets.
- `tag_template` — Image tag template. Defaults to `"{name}:{version}"`.
  `{name}` expands to the application name or `<name>-<subset>`;
  `{version}` expands to the application version.

```toml
[container]
base_image = "debian:bookworm-slim"
prefix = "/opt/portable-tar"
tag_template = "registry.internal.example.com/tools/{name}:{version}"
```
