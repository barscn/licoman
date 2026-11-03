# Contributing

Welcome. Licoman is phase-driven; read the latest handoff report before
starting anything — `docs/handoffs/PHASE_<NN>_REPORT.md`.

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

CI runs the manifest lint, the headless core suite and the full gpui
workspace suite on every push. No test touches a real dconf database or a
live session service; live verification runs are `#[ignore]`d,
env-gated, and require an explicit, user-approved operation list (see
`docs/live-testing/`).

## Adding a settings module (no Rust code needed)

See [`modules/CONTRIBUTING.md`](../modules/CONTRIBUTING.md) — the
Phase 08 proof module (`desktop-gnome-peripherals.toml`) was added as
manifest + docs alone.

## Adding a backend family

A new backend family needs: discovery rules, effective-state precedence,
writable capabilities (or an explicit read-only decision), concrete risk
limits, fixture tests, external-change handling, user-facing source
details, and verification on a real system when possible — the same
checklist Xfconf and mpv went through (see the Phase 08 report). Start
with an issue describing the target's documented public API.

## Reviews

Every change lands through a pull request. Safety-relevant changes (write
paths, privilege boundaries, isolation) additionally reference the threat
model (`docs/security/threat-model.md`) and expand it when new backend
capabilities appear.
