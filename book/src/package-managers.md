# Package managers

## mise

[mise](https://mise.jdx.dev/) can install Mitos's release archives and manage
multiple versions. The bundled runtime stays with each version's executable.

### GitHub backend

With mise installed, select Mitos globally:

```sh
mise use -g github:mitos-editor/mitos@0.1.0
mise exec -- ms --version
mise exec -- ms --health
```

Use `@latest` instead of `@0.1.0` to select the newest release. Omit `-g` to
record the version in the current project's `mise.toml` instead.
For `@latest`, mise's [minimum release age](https://mise.jdx.dev/configuration/settings.html#minimum_release_age)
defaults to 24 hours; pin a version to select a newly published release.
Once [mise is activated in your shell](https://mise.jdx.dev/getting-started.html#activate-mise),
you can run `ms` directly. The [GitHub backend](https://mise.jdx.dev/dev-tools/backends/github.html)
selects the archive for your operating system and CPU.

### Aqua backend

Mitos provides an [Aqua registry entry](https://github.com/mitos-editor/mitos/blob/main/contrib/aqua/registry.yaml)
for its release archives. Add this to your `mise.toml`:

```toml
[settings]
aqua.registries = ["https://raw.githubusercontent.com/mitos-editor/mitos/main/contrib/aqua/registry.yaml"]

[tools]
"aqua:mitos-editor/mitos" = "0.1.0"
```

Then install and check it:

```sh
mise install
mise exec -- ms --health
```

Use your [global mise configuration](https://mise.jdx.dev/configuration.html)
to make the tool available across projects. Append the registry URL to your
existing `aqua.registries` list if you already use custom registries.

This uses Mitos's custom registry; it does not require an entry in the shared
Aqua registry. See the [mise Aqua backend documentation](https://mise.jdx.dev/dev-tools/backends/aqua.html#custom-registry)
for configuration details. Both backends retain `runtime/` alongside `ms`,
so no `MITOS_RUNTIME` setting is needed for the bundled files.

## Debian package

The x86_64 release includes a `.deb` package. See
[Debian and Ubuntu installation](./install.md#debian-and-ubuntu).

## Other installation methods

See [Installation](./install.md) for standalone archives for all six supported
platforms, and [Building from source](./building-from-source.md) for Cargo and
local Debian package builds.
