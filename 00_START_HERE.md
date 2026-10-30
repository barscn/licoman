# Licoman — Start Here

**Licoman** — **Li**nux **Co**nfiguration **Man**ager. A universal interface
for Linux configuration, built with **Rust + GPUI + GPUI Kit**. No Electron,
no browser UI, no embedded web view.

> **Status: Phase 02 (production foundation) implemented.** The app runs on
> a UI-independent core with a deterministic mock provider. No real
> configuration is read or written yet — Licoman only ever writes its own
> preferences file. See
> [docs/handoffs/PHASE_02_REPORT.md](docs/handoffs/PHASE_02_REPORT.md) and
> [docs/architecture/README.md](docs/architecture/README.md).

## Developer commands

Run all of these from the repository root. All four are release gates —
`cargo fmt` / `cargo test` / `cargo clippy` / `cargo build --release` must
pass before any handoff is considered complete.

```sh
cargo fmt --all -- --check                        # formatting gate
cargo test --workspace                            # unit tests (headless)
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --release
cargo run -p licoman                              # run the app (GUI session)
```

Useful narrower commands:

```sh
cargo test -p licoman-core        # domain tests only (no GUI libs needed)
cargo run -p licoman -- --help    # CLI help
cargo run -p licoman -- --version
```

Linux runtime requirements: a Wayland or X11 session with GPU (Vulkan)
available, fontconfig fonts. Development on Arch tested with Hyprland
(Wayland). Without a graphical session the binary exits with a readable
diagnostic; domain tests run fine headless.

## Visual verification harness

`LICOMAN_DEMO` opens a deterministic screen without input injection:

| Value | Shows |
|---|---|
| `apps` | Applications window |
| `prefs` | Licoman Preferences window |
| `light` / `light-apps` | Light theme, main / Applications window |
| `failed` | Failed write state (Retry/Revert) |
| `distro` / `desktop` | Distribution / Desktop scope |
| `dirty` | One staged edit + apply bar |
| `palette` | Global search palette open |
| `empty` | No providers registered (unavailable state) |

```sh
LICOMAN_DEMO=apps cargo run -p licoman
```

Logs: `LICOMAN_LOG=info cargo run -p licoman` (structured key=value records
on stderr; ids and counts only — never setting values).

## Repository layout

```text
apps/licoman/        # executable: CLI, logging init, GPUI bootstrap
crates/core/         # licoman-core: UI-independent domain (no GPUI)
crates/ui/           # licoman-ui: Quiet Control themes + windows + views
docs/architecture/   # crate boundaries and decision records (D14+)
docs/design/         # Quiet Control design system documents (Phase 01)
docs/handoffs/       # per-phase reports (PHASE_01_REPORT.md, PHASE_02_REPORT.md …)
docs/screenshots/    # captured on a real Hyprland/Wayland session
tests/fixtures/      # strictly isolated fixtures (used from Phase 03 on)
tests/tools/         # dev verification harnesses (WIP Wayland key injector)
.github/workflows/   # CI: headless core job + full gpui workspace job
```

## Where to read next

1. [docs/architecture/README.md](docs/architecture/README.md) — crate
   boundaries, data flow, boundary rules.
2. [docs/architecture/decisions.md](docs/architecture/decisions.md) —
   Phase 02 decision records (D14–D24).
3. [docs/design/README.md](docs/design/README.md) — the approved design
   system (tokens, layout, states, accessibility).
4. [docs/handoffs/PHASE_02_REPORT.md](docs/handoffs/PHASE_02_REPORT.md) —
   what this phase did and how it was verified.
5. The authoritative execution plan is the author's document pack
   (`Licoman_Development_Phases/00_START_HERE.md`); exactly one phase per
   session, each ending with a handoff report.
