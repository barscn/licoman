# Contributing to Licoman

Thank you for looking at Licoman. This document explains how the project is
run, what the hard boundaries are, and how to verify a change.

## The project is phase-driven

Licoman is built in numbered phases (00–09 of the author's plan pack).
**Exactly one phase per working session**, each ending with
`docs/handoffs/PHASE_<NN>_REPORT.md`. Check the latest handoff report before
starting anything; do not skip ahead.

## Non-negotiable boundaries

Violating any of these is a bug even if tests pass:

1. **Existing configuration remains authoritative.** Licoman is an interface
   to native files/services, never a replacement store.
2. **`crates/core` never imports GPUI** (or any GUI library). The dependency
   graph enforces this; keep it that way.
3. **The UI never touches configuration directly.** Views dispatch typed
   `licoman_core::Command`s; only `licoman_core::prefs` writes to disk, and
   only to `$XDG_CONFIG_HOME/licoman/preferences.toml`.
4. **Least privilege.** The GUI always runs unprivileged. No `sudo`, no
   privileged helpers outside the (future, separately approved) safety
   engine.
5. **Fail closed.** Unknown/unreadable/uncertain ⇒ unavailable, read-only,
   or failed states. Never guess, never silently overwrite.
6. **No arbitrary scripting from manifests** — declarative, allowlisted
   providers only (relevant from Phase 04 on).
7. **Truthful UI.** Read-only, dirty, applying, applied, failed, external
   conflict, permission and restart-required are distinct states.
8. **No baseline telemetry, no network access, no background daemon, no
   idle polling.**
9. **No version bumps by guesswork.** Inspect `Cargo.lock` and the actual
   gpui-kit APIs first; pins are recorded in
   `docs/architecture/decisions.md`.
10. **Never log setting values, file contents, or credentials.** Logs carry
    ids, counts, and error kinds only (`licoman_core::log`).

## Development workflow

```sh
cargo fmt --all -- --check                        # must pass
cargo test --workspace                            # must pass, headless
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --release
```

For UI changes, verify visually on a real Linux session (Wayland or X11) and
use the `LICOMAN_DEMO` hooks (see [00_START_HERE.md](00_START_HERE.md)) for
deterministic captures. A Wayland screenshot is not proof of X11 support and
vice versa — say which session you tested.

## Where changes belong

| Change | Location |
|---|---|
| Domain logic, values, phases, validation | `crates/core` (+ unit tests) |
| Providers/backends | implement `licoman_core::Provider`; fixtures in `crates/core/src/mock.rs` |
| Rendering, themes, windows, keybindings | `crates/ui` |
| Startup, CLI, wiring | `apps/licoman` |
| Architecture changes | add a decision record in `docs/architecture/decisions.md` |

## Test fixtures

Anything under `tests/fixtures/` is strictly isolated synthetic data. Never
point a fixture or a test at real user configuration; never commit real
system files. Mock content must stay visibly labeled as mock in the UI.

## Reporting

When you finish a body of work, report truthfully: exact commands run, real
output (including failures), known gaps, and the security/data-loss
assessment. "It compiles" is not done.
