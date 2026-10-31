# Licoman Architecture

**Status:** as of Phase 04 (2026-10-31). Decision records live in
[decisions.md](decisions.md); design decisions from Phase 01 live in
[docs/design/decisions.md](../design/decisions.md) (D1–D13).

## Crate layout

Licoman is a **modular monolith**: one repository, three crates, dependency
arrows pointing one way only.

```text
apps/licoman          executable + GPUI bootstrap (CLI, logging init, wiring)
    │
    ▼
crates/licoman-ui     presentation adapter (GPUI Kit): themes, windows, views
    │
    ▼
crates/licoman-core   domain: typed values, descriptors, lifecycle store,
                      provider/registry abstractions, mock provider,
                      Licoman preferences, structured logging, window model
```

- `licoman-core` depends on **no GPUI** — in fact on no GUI library at all.
  Its dependencies are `serde`, `toml` (Licoman's own preferences file and
  the module manifests) and `schemars` (the generated module JSON Schema,
  Phase 04). This is enforced structurally by the dependency graph, not by
  convention: if a file under `crates/core` needs gpui, the build fails.
- Since Phase 04 the shipped settings catalog is **declarative**: built-in
  module manifests (`modules/builtin/*.toml`, embedded at build time) are
  validated, indexed and evaluated for applicability in
  `crates/core/src/modules/`; the authoring contract is documented in
  [../module-schema.md](../module-schema.md).
- `licoman-ui` renders the **Quiet Control** design system (tokens in
  `crates/ui/src/theme.rs`) and adapts core state to widgets.
- `apps/licoman` is deliberately thin: CLI parsing, environment checks,
  logging setup, and the gpui `application()` bootstrap.

## Boundary rules (non-negotiable, from 00_START_HERE.md)

1. **Core never imports GPUI.** Descriptors, values, phases, errors,
   validation, the store, providers, the registry, search and preferences are
   all presentation-free.
2. **The UI never touches configuration itself.** Views read owned snapshots
   (`Store::scope_rows`, `Store::model`, `SettingModel` fields) and dispatch
   typed [`Command`](../../crates/core/src/command.rs)s through
   `AppState::dispatch` — the single mutation path. No view opens `/etc`,
   invokes `gsettings`, or edits TOML files; only `licoman_core::prefs` may
   write, and only to Licoman's own preferences file.
3. **Providers own the truth.** The store holds *observations* of provider
   state; the source (file, daemon, backend) stays authoritative. Reloads
   accept the source value; Licoman never pretends a write stuck without a
   readback.
4. **Fail closed.** Unknown, unavailable, permission-held and conflicting
   settings render explicit states, never guesses.

## Data flow

```text
user intent (widget event)
  → typed licoman_core::Command
  → AppState::dispatch → Store::dispatch     [validation, phase transitions]
  → DispatchReport { outcome, changed areas }
  → AppEvent emitted; windows re-render from fresh snapshots
```

Staged applies are split into `Store::apply_begin` (validate + mark
Applying) and `Store::apply_complete` (write + readback), so the UI can show
a truthful "Applying…" interval without the store ever touching timers,
threads, or GPUI. There is **no background daemon and no idle polling**:
detection of external changes happens on explicit reloads and at load-time
verification; a real watch mechanism arrives with real backends and would be
event-driven, not polled.

## Where things live

| Concern | Module |
|---|---|
| Typed values + validation | `crates/core/src/value.rs` |
| Descriptors (identity, control, metadata) | `crates/core/src/descriptor.rs` |
| Lifecycle phases + readback | `crates/core/src/state.rs` |
| Store (models, dispatch, apply, reconcile) | `crates/core/src/model.rs` |
| Command vocabulary | `crates/core/src/command.rs` |
| Provider trait + capabilities | `crates/core/src/provider.rs` |
| Registry (routing by stable ID) | `crates/core/src/registry.rs` |
| Search index (stable IDs) | `crates/core/src/search.rs` |
| Mock provider + fixtures + apps catalog | `crates/core/src/mock.rs` |
| Licoman preferences (the only writable file) | `crates/core/src/prefs.rs` |
| Structured logging | `crates/core/src/log.rs` |
| Window single-instance/nav model | `crates/core/src/window_sessions.rs` |
| Quiet Control theme mapping | `crates/ui/src/theme.rs` |
| View adapter (store entity, events, timers) | `crates/ui/src/app_state.rs` |
| Windows (geometry, titles) | `crates/ui/src/windows.rs` |
| Main/Applications/Preferences windows | `crates/ui/src/ui/*.rs` |
| CLI (`--help`, `--version`) | `apps/licoman/src/cli.rs` |
| Bootstrap | `apps/licoman/src/main.rs` |

## Versioning

- `licoman_core::SETTINGS_MODEL_VERSION` — version of the in-memory settings
  type definitions (descriptors, values, phases). Bump on breaking changes.
- `licoman_core::prefs::SCHEMA_VERSION` — version of
  `$XDG_CONFIG_HOME/licoman/preferences.toml`. Loaders degrade to safe
  defaults with a diagnostic on unsupported or corrupt files.
