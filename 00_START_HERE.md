# Licoman — Start Here

**Licoman** — **Li**nux **Co**nfiguration **Man**ager. A universal interface
for Linux configuration, built with **Rust + GPUI + GPUI Kit**. No Electron,
no browser UI, no embedded web view.

> **Status: Phase 09 (release quality + packaging) implemented —
> release candidate **0.1.0-alpha.0** proposed, nothing published.** New:
> Vulkan startup diagnostics, an Arch PKGBUILD (makepkg-verified) with
> launcher/icons/AppStream under the  id,
> measured quality gates (first frame ≈ 0.6 s, idle CPU 0.07 %, search
> p95 ≈ 0.1 ms), the full release documentation set and a Go/No-Go
> report. See [docs/handoffs/PHASE_09_REPORT.md](docs/handoffs/PHASE_09_REPORT.md),
> [docs/installation.md](docs/installation.md) and
> [CHANGELOG.md](CHANGELOG.md) (decisions D59–D60).

> **Status: Phase 08 (desktop/application expansion + contributor
> workflow) implemented.** New Xfconf read backend (explicitly read-only,
> live-verified against real xfconfd) and mpv file-backend resolution
> (version-gated writes, live-verified reads), a manifest-only
> contributor-proof module, virtualized settings rendering measured at
> 1,240-setting scale, and the full contributor doc set
> ([modules/README.md](modules/README.md),
> [modules/CONTRIBUTING.md](modules/CONTRIBUTING.md),
> [docs/compatibility.md](docs/compatibility.md),
> [docs/security/module-trust.md](docs/security/module-trust.md)). See
> [docs/handoffs/PHASE_08_REPORT.md](docs/handoffs/PHASE_08_REPORT.md)
> and [docs/architecture/decisions.md](docs/architecture/decisions.md)
> (D55–D58).

> **Status: Phase 07 (first live settings integrations) implemented.**
> The Phase 06 safe write engine now has live backends: Alacritty TOML
> writes (version-validated, xattr/ACL preserve-or-refuse), GSettings
> writes through the **native** API behind a fail-closed GNOME desktop
> gate, and PowerProfiles property writes over the system bus — all gated
> by the same core-derived policies and only ever triggered by an explicit
> user Apply. A user-approved live GSettings round trip was verified on a
> disposable dconf root (real stack, real database untouched); an optional
> read-only Hyprland compositor-diagnostics module reads the documented
> IPC. See [docs/handoffs/PHASE_07_REPORT.md](docs/handoffs/PHASE_07_REPORT.md),
> [docs/architecture/integrations.md](docs/architecture/integrations.md),
> [docs/live-testing/PHASE_07_LIVE_CHECKLIST.md](docs/live-testing/PHASE_07_LIVE_CHECKLIST.md),
> [docs/architecture/write-engine.md](docs/architecture/write-engine.md) and
> [docs/architecture/decisions.md](docs/architecture/decisions.md) (D49–D54).

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
| `arch-hyprland`, `fedora-gnome`, `ubuntu-x11`, `debian-xfce`, `multi-xdg`, `unknown` | Discovery demo snapshots built through the real parsers (environments this machine cannot run) |
| `all-apps` | Applications window opened directly in the "All installed" view |
| `app-detail` | Applications window with an installed module-backed app selected (registry-driven tabs and rows) |
| `reads` | Phase 05 read backends on fixture data: real GSettings values with origin lines, plus the Source details dialog |
| `reads-power` | The power-profiles service absent: truthful per-row unavailability |
| `reads-apps` | Applications window: Alacritty rows read from the fixture TOML config |
| `writes` | Phase 06 safe write engine on a temp fixture copy + fake service: staged edits, apply bar (nothing outside the temp dir is touched) |
| `writes-conflict` | A staged row changed externally: conflict state with Reload/Compare, no overwrite |
| `writes-permission` | A permission-held row: lock, honest authorize dialog (deny-by-default broker) |
| `writes-light` | Light theme with the Phase 06 write states visible (theme-while-dialog verification) |

```sh
LICOMAN_DEMO=apps cargo run -p licoman
```

Environment diagnostics (headless, read-only, redacted):

```sh
cargo run -p licoman -- --diagnostics
```

Module tooling (Phase 04, both read-only):

```sh
cargo run -p licoman -- module validate modules/examples/   # manifest linter
cargo run -p licoman -- module schema                       # JSON Schema
```

Logs: `LICOMAN_LOG=info cargo run -p licoman` (structured key=value records
on stderr; ids and counts only — never setting values).

## Repository layout

```text
apps/licoman/        # executable: CLI, logging init, GPUI bootstrap
crates/core/         # licoman-core: UI-independent domain (no GPUI)
crates/ui/           # licoman-ui: Quiet Control themes + windows + views
docs/architecture/   # boundaries, decision records (D14+), discovery.md, backends.md
docs/design/         # Quiet Control design system documents (Phase 01)
docs/handoffs/       # per-phase reports (PHASE_01_REPORT.md, PHASE_02_REPORT.md …)
docs/screenshots/    # captured on a real Hyprland/Wayland session
tests/fixtures/      # strictly isolated fixtures (discovery fixtures: Phase 03)
modules/             # built-in + example module manifests (Phase 04)
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
4. [docs/architecture/discovery.md](docs/architecture/discovery.md) — how
   read-only environment discovery works (sources, precedence, limits).
5. [docs/architecture/backends.md](docs/architecture/backends.md) — the
   Phase 05 read-only backends (GSettings/dconf, TOML+imports, D-Bus).
6. [docs/architecture/write-engine.md](docs/architecture/write-engine.md) —
   the Phase 06 safe write engine (policies, conflict checks, backups).
7. [docs/security/threat-model.md](docs/security/threat-model.md) — assets,
   threats and mitigations for the write path.
5. [docs/handoffs/PHASE_03_REPORT.md](docs/handoffs/PHASE_03_REPORT.md) —
   what this phase did and how it was verified.
5. The authoritative execution plan is the author's document pack
   (`Licoman_Development_Phases/00_START_HERE.md`); exactly one phase per
   session, each ending with a handoff report.
