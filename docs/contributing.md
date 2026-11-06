# Contributing

Welcome. Start with [docs/architecture/README.md](architecture/README.md)
for the crate boundaries and [CONTRIBUTING.md](../CONTRIBUTING.md) for the
project's hard rules.

## The hard boundaries (violating any of these is a bug even if tests pass)

1. **Existing configuration remains authoritative.** Licoman is an
   interface to native files and services, never a replacement store.
2. **`crates/core` never imports GPUI** (or any GUI library); the
   dependency graph enforces it.
3. **The UI never touches configuration directly.** Views dispatch typed
   `licoman_core::Command`s; only the write engine writes, only through
   the documented pipeline.
4. **Least privilege.** The GUI always runs unprivileged; no sudo, no
   privileged helpers without a separately reviewed design.
5. **Fail closed.** Unknown/unreadable/uncertain ⇒ unavailable, read-only
   or failed states — never a guess.
6. **No arbitrary scripting from manifests.** Data-only allowlisted
   providers only; see `docs/security/module-trust.md`.
7. **Honest claims only.** Support levels, persistence labels and release
   notes state exactly what was demonstrated; tests assert this in the
   places that matter (fail-closed gates, log redaction).

## Development workflow

```sh
cargo fmt --all -- --check                        # formatting gate
cargo test --workspace                            # unit tests (headless)
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --release
cargo run -p licoman                              # the app (GUI session)
cargo run -p licoman -- --diagnostics             # headless facts
```

No test touches a real dconf database or a live session service; live
verification runs are `#[ignore]`d, env-gated, and require an explicit,
user-approved operation list. (The repository's CI workflow is temporarily
disabled — run the release gates locally as below.)

## Adding a settings module (no Rust code needed)

See [`modules/CONTRIBUTING.md`](../modules/CONTRIBUTING.md) — a module is
manifest + docs alone; `modules/builtin/desktop-gnome-peripherals.toml` is
the worked example.

## Adding a backend family

A new backend family needs: discovery rules, effective-state precedence,
writable capabilities (or an explicit read-only decision), concrete risk
limits, fixture tests, external-change handling, user-facing source
details, and verification on a real system when possible — the same
checklist Xfconf and mpv went through. Start with an issue describing the
target's documented public API.

## Reviews

Every change lands through a pull request. Safety-relevant changes (write
paths, privilege boundaries, isolation) additionally reference the threat
model (`docs/security/threat-model.md`) and expand it when new backend
capabilities appear.

## Deterministic demo screens

`LICOMAN_DEMO` opens a deterministic screen without input injection —
useful for screenshots and visual review. All values:

| Value | Shows |
|---|---|
| `apps` | Applications panel in the main window |
| `prefs` | Settings panel in the main window |
| `light` / `light-apps` | Light theme, Summary / Applications panel |
| `failed` | Failed write state (Retry/Revert) |
| `distro` / `desktop` | Distribution / Desktop scope |
| `dirty` | One staged edit + apply bar |
| `palette` | Global search palette open |
| `empty` | No providers registered (unavailable state) |
| `arch-hyprland`, `fedora-gnome`, `ubuntu-x11`, `debian-xfce`, `multi-xdg`, `unknown` | Discovery demo snapshots built through the real parsers (environments this machine cannot run) |
| `all-apps` | Applications panel opened directly in the "All installed" view |
| `app-detail` | Applications panel with an installed module-backed app selected (registry-driven tabs and rows) |
| `reads` | Read backends on fixture data: real GSettings values with origin lines, plus the Source details dialog |
| `reads-power` | The power-profiles service absent: truthful per-row unavailability |
| `reads-apps` | Applications panel: Alacritty rows read from the fixture TOML config |
| `writes` | Safe write engine on a temp fixture copy + fake service: staged edits, apply bar (nothing outside the temp dir is touched) |
| `writes-conflict` | A staged row changed externally: conflict state with Reload/Compare, no overwrite |
| `writes-permission` | A permission-held row: lock, honest authorize dialog (deny-by-default broker) |
| `writes-light` | Light theme with the write states visible (theme-while-dialog verification) |

```sh
LICOMAN_DEMO=apps cargo run -p licoman
```
