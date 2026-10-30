# Phase 03 Report — Environment, Service and Installed-Application Discovery

**Date:** 2026-10-31
**Workspace:** `/home/baris/Work/licoman` (`main`, Phase 02 commit + this
phase's uncommitted changes — nothing was committed or pushed by the
automated session, per the execution contract; the tree is ready for review).
**Executor:** automated session (ZCode), one phase per session as required.
**Plan source:** `03_ENVIRONMENT_DISCOVERY.md` (author's document pack). All
nine section headings of the required handoff format are below.

---

## 1. Goal and actual scope implemented

**Goal (from the plan):** "Produce **read-only, explainable capability
discovery** that determines which modules and controls may appear." Hard
boundary honored: no user, desktop, package-manager or system configuration
was edited; no `.desktop` `Exec` was executed; no daemon was launched.

### Task 1 — Distribution and system facts (`discovery/os_release.rs`)
- Hand-rolled os-release parser per the standard (`ID`, `ID_LIKE`, `NAME`,
  `VERSION_ID`, `PRETTY_NAME`): quoted values, comments, **missing,
  malformed and future fields tolerated** (unknown keys kept, malformed
  lines counted into an `os-release-malformed` problem). Matching is by IDs
  only (`matches_family` over `ID`+`ID_LIKE`), never pretty names.
- Kernel/arch from `/proc/sys/kernel/osrelease` + `std::env::consts::ARCH`,
  kept strictly informational (never conflated with distro identity).
- Package-manager availability (pacman/dnf/apt/zypper/apk/emerge) probed by
  PATH **stat** only, reported as informational facts — presence is not
  permission.
- Namespace/container context: `/proc/1/cgroup`, `/.dockerenv`,
  `/run/.containerenv`, WSL kernel markers → `ExecutionContext`
  (Host/Container/WSL) as evidence only, never a capability gate.

### Task 2 — Desktop/session facts (`discovery/session.rs`)
- All indicators combined: `XDG_CURRENT_DESKTOP` (**colon-split, per-element
  matching — no false single-string matches**; first element wins per the
  spec's preference order), `XDG_SESSION_DESKTOP` (Medium),
  `DESKTOP_SESSION` (Low, generic names ignored), `XDG_SESSION_TYPE` (High),
  `WAYLAND_DISPLAY`/`WAYLAND_SOCKET`/`DISPLAY` (Medium).
- GNOME / KDE Plasma / Xfce / COSMIC / Hyprland / Sway recognized; anything
  else is honestly `Unknown` (no completeness claim).
- Three separate facts: desktop identity, compositor/WM (a documented
  *knowledge table* — GNOME→Mutter etc. — not a probe), display protocol
  ("GNOME on Wayland" ≠ "GNOME").
- Disagreements keep **all candidates with per-source confidence** plus an
  `ambiguity` note — never one arbitrary winner. Both Wayland and X11
  sockets visible → nested/XWayland note; nothing crashes or assumes.

### Task 3 — Installed application enumeration (`discovery/apps.rs`)
- XDG data-directory precedence (user `$XDG_DATA_HOME` → `$XDG_DATA_DIRS`
  → Flatpak export dirs user+system); dedup by **desktop-file id**, first
  dir wins, shadowed duplicates counted; origin evidence per app
  (user/system/flatpak-user/flatpak-system).
- Desktop-entry semantics: `Type=Application` only, `Hidden` ignored per
  spec, `NoDisplay` kept+flagged (All-installed view only), `TryExec`
  resolved by `stat` against the PATH snapshot (missing → skipped with an
  explanatory problem), `OnlyShowIn`/`NotShowIn` evaluated against the
  current desktop (case-insensitive), `X-Flatpak` recorded.
- **`Exec` is never executed and never stored**; icon values are untrusted
  display metadata. Bounds: 1 MiB/file, regular files only (a planted FIFO
  or directory is skipped, never read), invalid UTF-8 and spec-invalid
  entries become isolated problems — one bad entry cannot stop the scan or
  the app.
- The five conditions stay separate: installed / module available /
  **version compatible (not claimed)** / backend available / **writable
  (not claimed)**. Module matching (`matching.rs`) is deterministic:
  desktop-id equality (High) or `StartupWMClass` (Medium), case-insensitive,
  suffix-stripped — no fuzzy heuristics.

### Task 4 — Backend/service capability probes (`discovery/backends.rs`,
`dbus.rs`, `gsettings.rs`)
- **D-Bus session bus**: a dependency-free client (SASL EXTERNAL, `Hello`,
  `ListNames`, 2 s timeouts, passive — activates nothing) — verified live
  against the real session bus (72 names). Wire layout validated
  **byte-for-byte against captured libdbus traffic** (see §3).
- **GSettings schemas**: `gschemas.compiled` parsed by a minimal GVDB reader
  (schema ids **and key names**, no hash lookup needed — list-item
  enumeration). Verified against the real system file (66 schemas) and a
  committed synthetic fixture. The `gsettings` CLI is deliberately not used
  (it can auto-activate `dconf-service`).
- **Anticipated config locations** for catalog apps (alacritty/kitty/mpv/
  nvim/zed) — stat only; this is what separates *backend available* from
  *module available* in the UI.

### Task 5 — Threading, refresh, cache (`ui/app_state.rs`, `environment_watch.rs`)
- Discovery runs **off the UI thread** (GPUI background executor), staged
  via `DiscoveryRun::run_next_stage` — the UI publishes **partial results
  after every stage** (footer shows distro the moment it is known, apps
  appear while backends still probe).
- **Cancellable**: `ScanControl` (AtomicBool) checked between stages and
  file batches; the Refresh button cancels the in-flight scan and restarts;
  a generation counter keeps stale scans from publishing.
- Cache = the snapshot + derived app/module list (safe metadata only),
  invalidated by explicit Refresh and by a **debounced (500 ms) inotify
  watcher** (`notify` crate in the UI crate only). A real bug was found and
  fixed here: menu-scanner *read* events (`Access(Open)`) initially caused
  constant rescans — the watcher now invalidates on Create/Modify/Remove
  only, and is silent at idle (verified: 0 refreshes in a 5 s idle run).
- Effective setting values are never cached (their contracts are Phase 05).

### Task 6 — UI
- Sidebar footer: grounded `Arch Linux · Hyprland · Wayland` summary, each
  part omitted if undetected, "Environment unknown" when nothing is known;
  loading/cancelled status line. `MOCK_ENVIRONMENT` removed.
- Applications window: **Supported / All installed** view switch (default =
  module-backed), text filter, **Refresh** button, honest loading state,
  per-row install-origin subtitles, module-only rows labeled "Module
  registered; application not detected", and unknown apps showing exactly
  "No Licoman integration available yet." with their facts — never fake
  settings. Selection is now **identity-based** (`SelectApp { id }`), stable
  across rescans.
- `Command::SelectApp { index }` → `{ id }` (D28) — indices shift on
  rescan; identities do not.
- Theme untouched: discovery never calls theme code; Dark default and
  user selection unaffected (light capture `p03-main-window-light.png`
  shows the same real footer).

### Task 7 — Diagnostics export + demo hooks
- `licoman --diagnostics` (headless, no window/session needed): prints the
  redacted export — distribution, session + evidence, package managers,
  application counts by origin, backend statuses, all problems. `$HOME`
  paths → `~`; no `Exec`, no icon values, no file contents (D30).
- `LICOMAN_DEMO` fixture names (`arch-hyprland`, `fedora-gnome`,
  `ubuntu-x11`, `debian-xfce`, `multi-xdg`, `unknown`) build snapshots by
  **running the real parsers over embedded fixture data** (D29) — visual
  verification of environments this machine cannot run. `all-apps` opens
  the Applications window directly in the All-installed view.

**Not implemented (explicitly out of scope per the plan):** real settings
reads (Phase 05), module manifests (04), writes (06–07), watch-based setting
change detection, version-compatibility/writable conditions, X11 runtime
testing (no X11 session available — see §6).

## 2. Changed/added files and public interfaces

```text
crates/core/src/discovery/        NEW module (10 files):
  mod.rs            EnvironmentSnapshot, Evidence/Confidence, ScanControl/
                    Progress, DiscoverySources, DiscoveryRun (staged),
                    discover(), summary(), redact_path, fixtures tests
  os_release.rs     os-release parser, kernel info, execution context
  session.rs        desktop/protocol detection with evidence + ambiguity
  apps.rs           desktop-entry parser + XDG scanner (precedence, dedup,
                    Hidden/NoDisplay/TryExec/OnlyShowIn/X-Flatpak)
  dbus.rs           minimal D-Bus client (marshal/parse pure + live probe)
  gsettings.rs      minimal GVDB reader for gschemas.compiled
  backends.rs       package managers, bus, schemas, config locations
  matching.rs       discovered apps × registered modules → AppListEntry
  export.rs         redacted diagnostics text
  demo_fixtures.rs  LICOMAN_DEMO snapshots via the real parsers
crates/core/src/{lib,command,model,model_tests}.rs
                    SelectApp { id } stable identity; discovery wiring
crates/ui/src/app_state.rs        EnvironmentState (cache), staged async
                    discovery, ScanState, AppEvent::EnvironmentChanged,
                    demo injection
crates/ui/src/environment_watch.rs  NEW: notify watcher, debounce, futures
                    channel bridge to the main-thread executor
crates/ui/src/ui/apps_window.rs   REWRITTEN: merged list, view switch,
                    refresh, loading, honest unsupported detail
crates/ui/src/ui/main_window.rs   live footer replaces MOCK_ENVIRONMENT
crates/ui/src/windows.rs          open_apps_window_returning_entity (demo)
crates/ui/{Cargo.toml,lib.rs}, Cargo.toml, Cargo.lock
                    notify + futures (ui only; core unchanged: serde+toml)
apps/licoman/src/{cli,main}.rs    --diagnostics; discovery/demo wiring
tests/fixtures/discovery/         NEW: os-release variants, application
                    trees (valid/broken/hidden/NoDisplay/TryExec/
                    OnlyShowIn/Link/duplicates/Flatpak), synthetic
                    gschemas.compiled + its XML source
docs/architecture/discovery.md    NEW: sources, precedence, limits
docs/architecture/decisions.md    D25–D30
00_START_HERE.md, README.md       status, commands, demo table, layout
docs/screenshots/p03-*.png        9 captures (§5)
docs/handoffs/PHASE_03_REPORT.md  this file
```

Key public interfaces:

- `licoman_core::discovery`: `EnvironmentSnapshot` (+`summary()`,
  `redact_path`), `DiscoverySources` (`from_environment()` + injectable
  fields), `DiscoveryRun` (`new/next_stage/run_next_stage/snapshot`),
  `discover(…) -> (snapshot, ScanStatus, ScanTimings)`, `ScanControl`,
  `ScanProgress/ScanStage`, `Evidence/EvidenceSource/Confidence`,
  `DetectionProblem/ProblemKind`, `DistributionEvidence`, `KernelInfo`,
  `ExecutionContext`, `SessionEvidence/DesktopIdentity/Protocol`,
  `InstalledApp/InstallOrigin/AppScanDir`, `BackendCapability/Kind/Status`,
  `AppListEntry/build_app_list`, `export::diagnostics_text`,
  `demo_fixtures::demo_snapshot`.
- `licoman_ui`: `AppEvent::EnvironmentChanged`, `AppState::{start_discovery,
  refresh_environment, ensure_environment_watcher, load_demo_environment,
  environment_snapshot, app_list, scan_state}`, `ScanState`,
  `environment_watch::start`, `windows::open_apps_window_returning_entity`,
  `AppsWindow::demo_show_all`.
- Binary: `--diagnostics`; `LICOMAN_DEMO` gains
  `arch-hyprland|fedora-gnome|ubuntu-x11|debian-xfce|multi-xdg|unknown|all-apps`.
- Changed: `Command::SelectApp { id: Option<String> }` (was `index`);
  `Store::selected_app() -> Option<&str>`; `MOCK_ENVIRONMENT` removed.

## 3. Decisions and deviations

Recorded in `docs/architecture/decisions.md` (D25–D30): **D25** hand-rolled
probes, core stays dependency-free (why `gsettings`-as-subprocess is
avoided: dconf auto-activation); **D26** `notify`+`futures` in the UI crate
only, watcher owned by the adapter; **D27** discovery cache in the adapter,
not the `Store`; **D28** `SelectApp` stable id; **D29** demo fixtures run
the real parsers; **D30** CLI diagnostics export.

**The D-Bus debugging story (worth recording):** the first live probe hung
(timeout on `Hello`). Three real protocol facts were learned and fixed:
(1) header bytes 4–8 are the **body** length (0 for no-argument calls), not
the total length; (2) the header-field layout had to be rewritten to match
reality — variant `g` signature **immediately** after the field-code byte,
value 4-aligned relative to the message, and `PATH`'s variant signature is
**`o`** (object path), not `s`; (3) replies carry a **SIGNATURE field**
(`g` value, alignment 1) and the bus emits **signals** (NameAcquired) before
a reply, so the client skips non-reply messages and matches `REPLY_SERIAL`.
The corrected layout was validated by capturing real libdbus `Hello` bytes
(reference client → recording fake bus) and asserting exact equality in a
unit test; the live probe then returned 72 bus names.

No deviations from the plan. One contingency from the plan was exercised in
the safe direction: the GVDB reader *worked* (verified against the real
system database), so no presence-only fallback was needed.

## 4. Tests/commands run, with real outcomes

All commands from the repository root; none skipped. Final state:

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | **pass** |
| `cargo test --workspace` | **pass — 132/132** (core 122, ui 5, cli 5), 0 failed, 0 ignored |
| `cargo clippy --workspace --all-targets -- -D warnings` | **pass** — 0 warnings (after fixing 15 lints incl. digit grouping, `is_multiple_of`, let-chains, `vec_init_then_push`) |
| `cargo build --workspace --release` | **pass**, 0 warnings |
| `./target/release/licoman --version` | `licoman 0.1.0` |
| `./target/release/licoman --diagnostics` | full redacted export on this machine (§5) |

Test coverage mapped to the plan's **required test matrix**:

| Fixture / case | Test | Result |
|---|---|---|
| Arch + Hyprland + Wayland → exact evidence | `arch_hyprland_wayland_full_snapshot` (fixtures) + session unit tests | pass |
| Fedora + GNOME + Wayland, probes separate | `fedora_gnome_wayland_with_separate_capability_probes` | pass |
| Ubuntu + GNOME + X11 | `ubuntu_gnome_x11_correctly_identified` | pass |
| Debian + Xfce — no conflation | `debian_xfce_facts_not_conflated` (+ module asserts nothing package-related leaks into session facts) | pass |
| Multi-value `XDG_CURRENT_DESKTOP` | `multi_value_xdg_current_desktop_has_no_false_single_string_match` | pass |
| Missing `/etc/os-release` | `missing_os_release_means_unknown_and_usable` (distro unknown, apps still scanned) | pass |
| Broken/malicious `.desktop` files | `broken_entries_are_skipped_with_diagnostics_others_survive`, `oversized_desktop_file_is_skipped_not_crashed_on`, `scan_ignores_non_regular_files_and_missing_dirs` | pass |
| Duplicate desktop entries | `duplicate_entries_resolve_by_directory_precedence` (deterministic identity, user dir wins) | pass |
| Flatpak app with no module | `flatpak_app_without_module_is_installed_but_unsupported` (present in All-installed, honest message) | pass |
| No D-Bus session | `no_session_bus_is_a_safe_unavailable_state`, `probe_without_bus_is_safe_unavailable`, `probes_without_bus_or_schemas_fail_closed` | pass |
| Long-running scan → cancel | `long_scan_cancels_between_files_and_reports_cancelled` (300 files, cancelled mid-scan via progress callback), `scan_is_cancellable_and_reports_partial` | pass |

Plus: parser round-trips (os-release quoting/malformed/future fields;
desktop-entry unescape, locale keys, action groups), D-Bus exact-byte
marshal vs captured libdbus, reply/error/SIGNATURE field parsing, string
array bodies with alignment padding, GVDB garbage rejection + real-file
verification + synthetic fixture enumeration, matching determinism, summary
composition, redaction.

**CI:** the first workflow run (Phase 02 push `d509b1a`) completed
**success** on `github.com/barscn/licoman` (checked via the public API —
no new workflow changes were needed this phase: the discovery tests run in
the existing headless `core` job via `-p licoman-core`).

**Runtime verification on the real session** (all with the release binary):
discovery ran off-thread (main thread never blocked — window responsive
during scans); logs carry ids/counts only. Watcher live-fire: 62 apps →
created a `.desktop` in a previously non-existent Flatpak export dir →
debounced refresh fired → 63 apps (files and dirs removed afterwards; the
pre-existing `db/` untouched). Headless launch diagnostic unchanged
(exit 1, readable message). `LICOMAN_DEMO=empty` unchanged.

## 5. Visual captures and manual verification instructions

All captures on the real session (Arch/Hyprland/Wayland, `grim` at the
window's exact geometry, floated/resized/focused via `hyprctl`),
**release binary**, in `docs/screenshots/`:

| File | Shows |
|---|---|
| `p03-main-window-dark.png` | Main window, Dark default, **real footer "Arch Linux · Hyprland · Wayland"** |
| `p03-main-window-light.png` | Light theme, same real footer (theme selection untouched by discovery) |
| `p03-apps-window-dark.png` | Applications window: "61 applications discovered · 5 with Licoman modules", Supported/All-installed switch, Refresh, module rows honestly marked "application not detected" |
| `p03-apps-all-installed.png` | **All installed** view: real 61 apps with Unsupported tags + install-origin subtitles (e.g. "Installed via user") |
| `p03-demo-fedora-gnome.png` | Fixture demo: "Fedora Linux 43 (Workstation Edition) · GNOME · Wayland" |
| `p03-demo-ubuntu-x11.png` | "Ubuntu 26.04 LTS · GNOME · X11" |
| `p03-demo-debian-xfce.png` | "Debian GNU/Linux trixie (testing) · Xfce · X11" |
| `p03-demo-multi-xdg.png` | Ambiguous indicators: "Arch Linux · KDE Plasma · Wayland" (highest-confidence candidate; ambiguity recorded in snapshot/diagnostics) |
| `p03-demo-unknown.png` | "Environment unknown" — honest fallback |

Reproduce:

```sh
cargo run -p licoman                          # real discovery, dark
LICOMAN_DEMO=apps cargo run -p licoman        # Applications window (live)
LICOMAN_DEMO=all-apps cargo run -p licoman    # All-installed view
LICOMAN_DEMO=fedora-gnome cargo run -p licoman  # fixture matrix rows
cargo run -p licoman -- --diagnostics         # redacted headless export
LICOMAN_LOG=info cargo run -p licoman         # discovery logs (ids/counts only)
```

`licoman --diagnostics` on this machine (abridged): `id=arch`,
kernel `7.2.11-arch1-1 x86_64`, context host; `desktop=Hyprland
confidence=high`, protocol Wayland, Wayland+X11 ambiguity note; pacman
found; **61 applications** (user 7, system 54), 9 NoDisplay, 1 restricted;
`dbus-session: available (72 names)`; `gsettings-schemas: available (66
schemas)`; config probes mpv/neovim available; **1 real problem found**:
`qemu.desktop` skipped ("missing Exec") — a genuinely broken entry on this
system, isolated exactly as designed.

Human-only checks (unchanged limitations, no input-injection tool): the
keyboard-only walkthrough (Tab order, Ctrl+K, Esc), clicking the Refresh
button while watching for freezes (the off-thread design is verified by
logs, not by injection), live mid-run System-mode toggle.

## 6. Platform/environment matrix actually tested

| Dimension | Value |
|---|---|
| OS | Arch Linux (kernel 7.2.11-arch1-1), container context: host |
| Session | Hyprland (Wayland), `WAYLAND_DISPLAY=wayland-1`; both Wayland and X11 sockets visible (XWayland) — ambiguity note working as designed |
| Discovery (live) | 61 apps (7 user / 54 system), pacman, D-Bus 72 names, 66 GSettings schemas, config probes mpv+neovim |
| Rendering | Vulkan (mesa/RADV), gpui blade backend |
| Rust | 1.99.1 stable, edition 2024; new deps: `notify 8.2.1`, `futures 0.3` (UI crate only) |
| Fixture-verified rows | Fedora 43+GNOME+Wayland, Ubuntu 26.04+GNOME+X11, Debian trixie+Xfce (embedded-fixture demos + parser tests) |
| X11 / other DEs natively | **not tested** (single-session machine) — the plan accepts fixture-based verification for these rows |
| Headless | `--diagnostics` runs with no graphical session; GUI launch exits 1 with the unchanged diagnostic |

## 7. Security and data-loss assessment

- **Read-only, enforced structurally.** Discovery I/O is: read
  `/etc/os-release` + `/proc`, `read_dir`+`read_to_string` over XDG
  application dirs, `stat` over PATH/config dirs, one Unix-socket
  conversation with the session bus (Hello+ListNames, then disconnect), read
  `gschemas.compiled`. No writes anywhere; the only file Licoman can still
  create remains `$XDG_CONFIG_HOME/licoman/preferences.toml`.
- **No execution of discovered data.** `Exec` lines are never spawned and
  never stored on `InstalledApp`; `TryExec` is a `stat`; entries with
  FIFO/device/dir shape are skipped unread (1 MiB cap); invalid UTF-8 is a
  problem entry. The UI layer still cannot touch configuration; the new
  core I/O lives behind `licoman_core::discovery` with pure/injected
  parsing split.
- **Privacy.** Only the current user's XDG dirs and system data dirs are
  traversed — no other user's home. Logs: ids/counts/error kinds only.
  The diagnostics export redacts `$HOME` → `~` and excludes `Exec`/icon
  payloads/contents.
- **No daemon, no polling, no network.** The D-Bus probe connects,
  asks, disconnects — activates nothing. The watcher is event-driven
  (inotify; zero wakeups at idle — verified). No telemetry.
- **Host untouched during verification.** The watcher test created files
  only under a previously non-existent `~/.local/share/flatpak/exports/…`
  subtree and removed exactly that subtree afterwards (pre-existing `db/`
  verified untouched). No package, desktop or system configuration was
  read-modified or written at any point.

## 8. Remaining work and acceptance gates

| Acceptance criterion (03_ENVIRONMENT_DISCOVERY.md) | Status |
|---|---|
| Real distro + session + installed applications detectable without modifying host state | **pass** — live capture `p03-main-window-dark.png`; §5/§6 evidence |
| Unknown/missing/malformed inputs are not confused with supported values | **pass** — os-release missing/malformed tests, Unknown desktop identity, malformed entries skipped with problems |
| Capability results are evidence-based and deterministic for fixture inputs | **pass** — every fact carries Evidence+Confidence; `discovery_is_deterministic_for_fixture_inputs` |
| Application enumeration does not execute `.desktop` commands | **pass** — `Exec` never stored/spawned (asserted by test); stat-only TryExec |
| Discovery is cancellable and doesn't freeze GPUI rendering | **pass** — staged background-executor execution with ScanControl + generation guard (unit-tested cancellation); UI remained interactive in all captures |
| Dark remains default and user theme selection is untouched | **pass** — discovery has no theme path; dark-default and light captures show identical selection semantics |
| All mandatory tests, format/lint/build checks and documentation are reported | **pass** — 132/132 tests, fmt/clippy/release all green; discovery.md + D25–D30 + this report |

Known gaps / carried-over gates (pre-existing or explicitly deferred):
- Keyboard-only walkthrough remains a human gate (no input-injection tool;
  user-deprioritized since Phase 01).
- X11 and non-Hyprland desktops verified via fixtures/parsers, not native
  sessions (single-session machine; plan accepts this).
- GVDB reader is little-endian only (both supported targets are LE;
  big-endian files fail closed as "unreadable" with a problem entry).
- The watched directory set is fixed at startup; application directories
  created *after* launch are picked up on the next Refresh (documented in
  discovery.md).

**Stop condition:** this is the end of Phase 03. Per the plan — "Stop here.
Do not load user-authored executable plugins or start editing application
settings." Next session starts Phase 04 from this report.

## 9. Reverting this phase

No user configuration is at risk: discovery wrote nothing (the watcher-test
scratch subtree was removed during verification — see §7).

To revert the repository to the pre-Phase-03 state:

```sh
cd /home/baris/Work/licoman
git checkout -- . && git clean -fd crates/core/src/discovery crates/ui/src/environment_watch.rs docs/architecture/discovery.md docs/screenshots/p03-* tests/fixtures
# keeps Phase 02 (commit d509b1a) exactly as pushed
```

Nothing outside the workspace was modified; no commits or pushes were made.
