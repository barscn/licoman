# Privacy

**Licoman has no telemetry, no accounts, no cloud and no background
network access.** There is nothing to opt out of because there is nothing
sending data anywhere.

## What Licoman reads

- Files it displays: configuration files and platform databases
  (GSettings/dconf databases, Xfconf channels, D-Bus service properties)
  for the settings you actually open in the UI.
- Environment facts at startup (read-only, bounded): os-release fields,
  the session's desktop identity, installed application entries, the list
  of bus names on the session/system bus, presence of specific schemas and
  binaries.

## What Licoman writes

- Its own preferences file:
  `$XDG_CONFIG_HOME/licoman/preferences.toml` (theme and window state).
- Backups of configuration files immediately before an explicit,
  verified write: `$XDG_DATA_HOME/licoman/backups/` (mode 0700/0600,
  bounded retention). Deleting that directory removes the backups; your
  configurations themselves are never touched without the write pipeline.
- Nothing else. No service is started, stopped or installed; no package
  action is taken; no process other than Licoman is spawned (the only
  exceptions are the documented `--version` probes for Alacritty and mpv
  during application detection).

## What leaves the machine

Nothing. No crash reports, no usage statistics, no update checks that
phone home, no "anonymous analytics" — the code has no network client
beyond the D-Bus connections to your own session/system buses.

## Logs

Logs go to stderr, enabled with `LICOMAN_LOG=info` (or `debug`). They
record identifiers, redacted targets (paths collapse `~`), outcomes and
timings — **never setting values**. A test in the write-engine suite
plants a secret-looking value and asserts it cannot appear in any log or
operation record; that test runs in CI on every push.

## Third-party components

See [NOTICES](../NOTICES.md) for the dependency inventory. None of them
adds a network service to Licoman.
