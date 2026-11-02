# Licoman

**Licoman** — **Li**nux **Co**nfiguration **Man**ager. A universal interface for Linux configuration: one native desktop application over shared/system, desktop-environment, distribution and installed-application settings.

Built with **Rust + GPUI + GPUI Kit**. No Electron, no browser UI, no embedded web view.

> **Status: Phase 07 (first live settings integrations) implemented.**
> The three vertical slices are real: **Alacritty** (version-validated TOML
> writes with xattr/ACL preserve-or-refuse), **GNOME GSettings** (native
> libgio writes behind a fail-closed desktop gate, real reset semantics,
> native change notifications), and **power-profiles over D-Bus** (typed
> property writes with honest persistence). A live user-approved GSettings
> write round trip was verified against a disposable dconf database (real
> native stack, real database untouched). Every write still requires an
> explicit user Apply behind the core-derived policy gates — startup,
> discovery, tests and CI never write. See
> [docs/handoffs/PHASE_07_REPORT.md](docs/handoffs/PHASE_07_REPORT.md),
> [docs/architecture/integrations.md](docs/architecture/integrations.md),
> [docs/live-testing/PHASE_07_LIVE_CHECKLIST.md](docs/live-testing/PHASE_07_LIVE_CHECKLIST.md)
> and [docs/architecture/decisions.md](docs/architecture/decisions.md) (D49–D54).

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
cargo test --workspace         # unit tests (domain, preferences, theme, CLI)
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --release
cargo run -p licoman           # run the app (Wayland or X11 session)
```

Linux runtime requirements: a Wayland or X11 session with GPU (Vulkan) available, fontconfig fonts. Development on Arch tested with Hyprland (Wayland). Without a graphical session the binary exits with a readable diagnostic; `licoman-core` tests run headless.

### Visual verification harness

`LICOMAN_DEMO` selects a deterministic screen for screenshots (full table in
[00_START_HERE.md](00_START_HERE.md)): `palette`, `apps`, `prefs`, `light`,
`light-apps`, `failed`, `distro`, `desktop`, `dirty`, `empty`, plus
discovery fixtures (`arch-hyprland`, `fedora-gnome`, `ubuntu-x11`,
`debian-xfce`, `multi-xdg`, `unknown`, `app-detail`). Example:

```sh
LICOMAN_DEMO=apps cargo run -p licoman
```

Headless commands (all read-only): `--diagnostics` (redacted environment
export), `module validate <path>...` (manifest linter), `module schema`
(JSON Schema for module authors).

## Layout

```text
apps/licoman/        # executable: CLI, logging init, GPUI bootstrap
crates/core/         # licoman-core: UI-independent domain (no GPUI)
crates/ui/           # licoman-ui: Quiet Control themes, windows, views
docs/architecture/   # crate boundaries + Phase 02+ decision records
docs/design/         # Quiet Control design system documents
docs/handoffs/       # per-phase reports (PHASE_01_REPORT.md …)
docs/screenshots/    # captured on a real Hyprland/Wayland session
modules/             # built-in + example module manifests (Phase 04)
tests/fixtures/      # strictly isolated fixtures (used from Phase 03 on)
tests/tools/         # dev verification harnesses (WIP Wayland key injector)
.github/workflows/   # CI: headless core job + full gpui workspace job
```

Future boundaries (`crates/discovery`, `crates/backends`, `crates/safety`, …)
arrive with the phases that need them; Phase 02 established the three-crate
modular monolith described in
[docs/architecture/README.md](docs/architecture/README.md). See the execution plan in
`Licoman_Development_Phases/00_START_HERE.md` (author's document pack).
