# Container Settings

`[container]` configures OCI image output.

```toml
[container]
base_image = "gcr.io/distroless/base-debian12"
prefix = "/opt/env"
tag_template = "registry.internal.example.com/analytics/{name}:{version}-py311"
```

- `base_image` (optional) defaults to `gcr.io/distroless/base-debian12`.
- `prefix` (optional) relocates the Conda environment inside the image. Supply
  an absolute path; the default is `/opt/conda`.
- `tag_template` (optional) renders the final tag. Only `{name}` and
  `{version}` are recognised placeholders. The default template is 
  `{name}:{version}`.

Container builds emit `<name>-container.oci.tar` alongside the manifest unless
you override the output location on the command line.
