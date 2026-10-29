# Licoman

**Licoman** — **Li**nux **Co**nfiguration **Man**ager. A universal interface for Linux configuration: one native desktop application over shared/system, desktop-environment, distribution and installed-application settings.

Built with **Rust + GPUI + GPUI Kit**. No Electron, no browser UI, no embedded web view.

> **Status: Phase 01 (design system on mock data) implemented.** No real
> configuration is read or written yet. See
> [docs/handoffs/PHASE_01_REPORT.md](docs/handoffs/PHASE_01_REPORT.md).

## Principles (non-negotiable)

1. Existing configuration remains authoritative — Licoman is an interface, not a store.
2. UI-independent core (enforced from Phase 02).
3. Least privilege; the GUI always runs unprivileged.
4. Fail closed: unknown → unavailable/read-only, never a guess.
5. No arbitrary scripting from manifests.
6. Version-aware support across distro / session / DE / app dimensions.
7. Truthful UI states (read-only, dirty, applying, failed, external change…).
8. Licoman's own theme preference never touches OS appearance settings.
9. No baseline telemetry.
10. No version bumps by guesswork.

## Development

```sh
cargo fmt --all -- --check     # formatting gate
cargo test --workspace         # unit tests (preferences, theme mapping, metrics)
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --release
cargo run -p licoman           # run the app (Wayland or X11 session)
```

Linux runtime requirements: a Wayland or X11 session with GPU (Vulkan) available, fontconfig fonts. Development on Arch tested with Hyprland (Wayland).

### Visual verification harness

`LICOMAN_DEMO` selects a deterministic screen for screenshots (documented in
the phase report): `palette`, `apps`, `prefs`, `light`, `light-apps`,
`failed`, `distro`. Example:

```sh
LICOMAN_DEMO=apps cargo run -p licoman
```

## Layout

```text
apps/licoman/        # executable, GPUI bootstrap (Phase 01 shell)
docs/design/         # Quiet Control design system documents
docs/handoffs/       # per-phase reports (PHASE_01_REPORT.md …)
docs/screenshots/    # captured on a real Hyprland/Wayland session
tests/fixtures/      # strictly isolated fixtures (used from Phase 03 on)
```

Planned crate boundaries (`crates/core`, `crates/registry`, `crates/backends`,
`crates/safety`, …) arrive with the phases that need them; Phase 01 keeps a
single focused crate. See the execution plan in
`Licoman_Development_Phases/00_START_HERE.md` (author's document pack).
