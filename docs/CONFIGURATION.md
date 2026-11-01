# Pelagos startup configuration

Pelagos reads an **optional, read-only JSON** configuration file when starting the HTTP daemon. This is a private Pelagos configuration, not Jellyfin configuration. No existing file is automatically created or rewritten.

## File location and schema

Pelagos checks `$XDG_CONFIG_HOME/pelagos/config.json` when `XDG_CONFIG_HOME` is an absolute directory. Otherwise, an absolute `HOME` selects `$HOME/.config/pelagos/config.json`. If no configuration file exists, Pelagos preserves the earlier CLI defaults: bind to `127.0.0.1`, port `8097`, and **no UDP discovery**. An explicit `--config /path/to/config.json` must exist and pass validation.

```json
{
  "schema_version": 1,
  "bind": "127.0.0.1",
  "port": 8097,
  "discover": false
}
```

Only these four keys are accepted. The file is bounded to 16 KiB. `bind` must be an IP address literal; discovery is rejected for loopback addresses. Future `schema_version` values are rejected without touching state or credentials. Legacy unversioned JSON, or a `schema_version` of `0`, is interpreted as the equivalent version-1 settings **in memory only**, with no automatic filesystem migration or rewriting.

## CLI precedence

The existing `--bind`, `--port`, and `--discover` flags override the file. `--no-discover` explicitly disables discovery even when a file enables it, and cannot be combined with `--discover`. The existing `--state-dir` still controls private Pelagos state independently. Media/account maintenance subcommands do not need to load the optional server startup configuration.

## Verification and limitations

Unit tests in `src/config.rs` check absolute XDG-path selection, safe defaults, validation, future-version rejection, in-memory legacy normalization and override precedence. Isolated Linux subprocess regressions in `tests/config_cli.rs` verify a server can start on an explicitly overridden loopback address with discovery disabled, and that invalid explicit settings fail before private state is created.

These tests do not prove safe LAN exposure, systemd deployment, arbitrary prior Pelagos configuration migration, or any unmodified Jellyfin client acceptance. Never point Pelagos state at an existing Jellyfin database.
