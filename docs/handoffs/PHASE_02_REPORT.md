# Phase 02 Report — Production Foundation and GPUI Application Shell

**Date:** 2026-10-30
**Workspace:** `/home/baris/Work/licoman` (`main` @ `6852c1d` + this phase's
uncommitted changes — nothing was committed or pushed by the automated
session, per the execution contract; the tree is ready for review).
**Executor:** automated session (ZCode), one phase per session as required.
**Plan source:** `02_FOUNDATION.md` (author's document pack). All nine
section headings of the required handoff format are below.

---

## 1. Goal and actual scope implemented

**Goal (from the plan):** "Establish a maintainable Licoman application
without introducing real configuration edits."

**Implemented — task by task:**

### Task 1 — Inspect, preserve and choose migration strategy
- The plan's referenced prototype `barscn/gpui-kit-mock` is **not available
  on this machine** (established in Phase 01, decision D1); there is no
  separate prototype to preserve or overwrite. The Phase 01 implementation
  inside this repository *is* the working mock and is preserved as the
  regression reference: its files were **git-moved** (history-preserving
  renames) into the new layout, its stable setting IDs are unchanged, its
  fixtures are ported 1:1 into the mock provider, and its Phase 01
  screenshots/tests remain in the tree.
- Version/renderer/toolchain decisions re-verified and recorded (D15):
  gpui-kit **0.8.1** / gpui-pre **0.3.10** / ashpd **0.13.13** (locked),
  Rust **1.99.1 stable**, edition **2024**, renderer **blade/Vulkan**. No
  upgrade was attempted — the working pins stand (00_START_HERE.md
  non-negotiable 10).

### Task 2 — Modular monolith (the core of this phase)
Three-crate workspace (D14/D16), replacing the Phase 01 single crate:

```text
apps/licoman          bootstrap: CLI, logging init, GPUI wiring  (thin)
  └─ licoman-ui       GPUI presentation adapter (themes, windows, views)
       └─ licoman-core   UI-independent domain — NO GPUI dependency
```

- **`crates/core` (licoman-core)** — deps: `serde`, `toml` only. Contains:
  - `value.rs` — typed values **bool / string / integer / float / enum
    (choice) / path / list** (path & list are the optional-capable types;
    empty = not set) + descriptor-constrained validation.
  - `descriptor.rs` — `SettingDescriptor` (stable `id`, scope, category,
    label, description, type, control, **apply mode metadata**
    immediate|staged, restart target, permission scope, provider note,
    numeric bounds), `Scope::{Shared,Desktop,Distribution}`, `Control`.
  - `state.rs` — explicit lifecycle: **Unknown, Unavailable, Idle, Dirty,
    Validating, Pending, Applying, Applied, Failed, ExternalConflict** plus
    the Readback record — the diagram from 02_FOUNDATION.md §4, including
    Cancel/Discard, Failure, Readback and External-conflict→Reconcile.
  - `model.rs` — the `Store`: loaded `SettingModel`s, **centralized dirty
    tracking**, revert/reset semantics, validation at apply, staged
    apply-begin/apply-complete split, immediate synchronous apply,
    retry/revert/reload, readback (mismatch ⇒ conflict), window selections,
    Licoman preferences, demo hooks. No GPUI, no threads, no timers.
  - `command.rs` — typed `Command` vocabulary (`Edit, ApplyAll, DiscardAll,
    Discard, Retry, Revert, Reload, Select*, SetAppearance`) →
    `DispatchReport { outcome, changed areas }`. The UI dispatches commands;
    it never mutates domain state directly.
  - `provider.rs` — `Provider` trait: `probe` / `descriptors` / `read` /
    optional `write` + `Capabilities{read,write,watch}`. **No `watch`
    method exists in this phase** — no daemon, no polling (D17);
    `Capabilities::watch` is reserved.
  - `registry.rs` — registry routing reads/writes by **stable setting ID**.
  - `search.rs` — search index built from descriptors (not the visible
    scope), so result identities are stable across selection changes.
  - `mock.rs` — **deterministic mock provider**: fixture builder with read
    sequences (simulates "source changed between reads" → external
    conflict), unavailable/permission/read-only states, failing writes; the
    Phase 01 sample catalog and applications list ported unchanged (IDs
    identical). Drives search + the full lifecycle with no installed
    software.
  - `prefs.rs` — moved verbatim from Phase 01: Licoman-only preferences at
    `$XDG_CONFIG_HOME/licoman/preferences.toml` (atomic write, `$HOME`
    fallback), schema-version + mode-string validation, corrupt-safe load
    with diagnostics. Kept **separate from the registry and provider
    settings** (D21).
  - `log.rs` — structured, level-filtered (`LICOMAN_LOG`, default `warn`)
    key=value records on stderr with dependency-free RFC 3339 UTC
    timestamps; privacy rule enforced by design: ids/counts/error kinds
    only — **never values, file contents or credentials** (D22).
  - `window_sessions.rs` — headless model of single-instance window
    behavior and **independent per-window navigation histories** (D23).
- **`crates/ui` (licoman-ui)** — deps: `licoman-core`, `gpui-kit`, plus the
  ashpd feature-unification pin. Moved from Phase 01: `theme.rs` (Quiet
  Control tokens, unchanged), `actions.rs`, `windows.rs`, all three window
  views. Rewritten: `app_state.rs` is now a **view adapter** — it owns the
  core `Store` inside a GPUI entity, forwards typed commands, emits events,
  drives the visible 700 ms apply interval + 1800 ms "Saved" acknowledgment,
  applies themes to every window, and delegates open/focus/history decisions
  to core `WindowSessions`. Views render from owned snapshots
  (`Store::scope_rows`) and dispatch commands only.
- **`apps/licoman`** — bootstrap + `cli.rs` (`--help`, `--version`,
  argument errors; hand-rolled, D22). The ashpd/serde/toml pins moved out of
  the binary crate to where they belong.

### Task 3 — Production app shell
- Binary `licoman`, visible name **Licoman**; `--version` → `licoman 0.1.0`;
  `--help` full usage incl. environment/notes; unknown arguments → stderr
  error, exit 2.
- **Meaningful startup error handling:** without any graphical session
  (`WAYLAND_DISPLAY`/`WAYLAND_SOCKET`/`DISPLAY` all unset) the app logs a
  structured error and exits 1 with a readable diagnostic instead of opening
  a broken window (verified on the release binary).
- Main window with **Shared / Desktop / Distribution** scopes and
  **Applications** opening/focusing the second native window — carried over
  and now store-driven. Scope, category and app-window selections live in
  the store and are preserved independently (unit tested).
- **Search architecture:** global palette (Ctrl+K) searches **all** scopes
  via the registry-wide index with stable IDs + breadcrumbs; the main-window
  header search is the current-window entry (Enter opens the palette with
  the query); the Applications window filter narrows the app list
  (window-local). The mock provider drives all of it.
- Preferences / Appearance: **Dark (default) / Light / System** radio group —
  the Phase 01 implementation was integrated, **not duplicated** (prefs.rs
  moved into core; the preferences window renders from the store and
  dispatches `SetAppearance`).
- Same theme in every native window (single `Theme::change` refresh); theme
  resolved **before** the first window paints — no flash; `System` resolves
  via the desktop appearance signal with a disclosed Dark fallback.
- **Useful in an unsupported environment:** a registry with no providers
  shows an explicit "No settings available here" state (screenshot
  `p02-empty-state-dark.png`), never a blank window; unavailable settings
  show reasons; the headless-launch diagnostic covers the no-session case.

### Task 4 — Core state and action lifecycle
- Full state machine implemented in core (see Task 2) with the exact
  transition flow from the plan, including **Validating** and **Pending**
  steps, **Readback** after write, and **Reconcile** via Reload.
- **Immediate vs staged apply is descriptor metadata** (`ApplyMode`), unit
  tested both ways; no mock fixture is immediate — the real immediate flow
  in the product is the Licoman theme preference, which is deliberately
  **not** a provider setting: `Command::SetAppearance` performs the safe
  immediate write to Licoman's own config through the core (D21), distinct
  from external edits.
- Dirty tracking, discard, revert and reset are **centralized in core** —
  no view owns lifecycle state anymore.
- A control change only ever marks a row Dirty; persistence happens
  exclusively through the apply path with provider readback.
- **No background daemon, no idle polling, no unmanaged timers.** External
  changes are detected by the load-time verification read and explicit
  reloads (D19); the only timers run while an apply/acknowledgment is in
  flight.

### Task 5 — Project standards
- Structured logging (above), privacy rule documented in `CONTRIBUTING.md`.
- Deterministic fixtures (mock provider + read sequences);
  `SETTINGS_MODEL_VERSION` + `prefs::SCHEMA_VERSION` versioning.
- New: repo-level **`00_START_HERE.md`** (developer commands incl. fmt/test/
  lint/release, demo hooks table, layout), **`CONTRIBUTING.md`** (workflow,
  hard boundaries, fixture rules, truthful reporting), **README.md** updated
  (status, layout, commands), **`.github/workflows/ci.yml`** — two jobs:
  headless `core` (fmt/clippy/test, no system deps) and full `workspace`
  (gpui system packages per upstream gpui-kit's own Linux install script,
  clippy/test/release build + CLI smoke); GPU/session-required checks are
  deliberately not in CI and are documented as human-run.
- Security boundary notes in `CONTRIBUTING.md`, `00_START_HERE.md`,
  `docs/architecture/`.
- No JavaScript, WebView, browser shell, Electron, or remote service — the
  dependency set is gpui-kit + serde + toml (+ ashpd pin), unchanged.

**Not implemented (explicitly out of scope per the plan):** real discovery
(Phase 03), module manifests (04), real read backends (05), writes (06–07),
watch support, collapsible sidebar (Phase 01 D-deferral stands), icon
artwork, X11 runtime testing (no X11 session available — see §6).

## 2. Changed/added files and public interfaces

```text
Cargo.toml / Cargo.lock         workspace: 3 members, licoman-core/ui path deps
00_START_HERE.md                NEW developer entry point (commands, demos)
CONTRIBUTING.md                 NEW contribution + boundary rules
.github/workflows/ci.yml        NEW CI (core headless job, gpui workspace job)
README.md                       status/layout/commands updated
apps/licoman/Cargo.toml         deps now licoman-core, licoman-ui, gpui-kit
apps/licoman/src/main.rs        REWRITTEN: CLI dispatch, session check, log
                                init, store construction, demo hooks
apps/licoman/src/cli.rs         NEW: --help/--version/args (pure, tested)
apps/licoman/src/mock.rs        REMOVED (fixtures re-expressed in core mock)
crates/core/                    NEW crate (12 modules, 50 unit tests)
crates/ui/                      NEW crate (moved views + rewritten adapter)
docs/architecture/README.md     NEW boundaries, data flow, module map
docs/architecture/decisions.md  NEW D14–D24
docs/handoffs/PHASE_02_REPORT.md  this file
docs/screenshots/p02-*.png      11 new captures (§5)
```

Key public interfaces (crate-level):

- `licoman_core`: `Store` (`new/dispatch/apply_begin/apply_complete/
  load/scope_rows/search/model/descriptor/effective_value/pending_count/
  preferences/…`), `SettingModel`, `Command`, `DispatchReport`, `Outcome`,
  `SettingValue`, `SettingType`, `SettingDescriptor`, `Control`, `Scope`,
  `ApplyMode`, `SettingPhase`, `Readback`, `LicomanError`, `ErrorKind`,
  `Provider`, `Capabilities`, `ProbeResult`, `Registry`, `search()`,
  `WindowSessions`/`WindowRole`, `MockProvider`, `APPS_CATALOG`,
  `prefs::{AppearanceMode, Preferences, load/save/load_from/save_to/
  LoadDiagnostic}`, `log`, `SETTINGS_MODEL_VERSION`.
- `licoman_ui`: `AppState` (`dispatch/change_theme/complete_apply/
  resolve_and_apply_theme/push_history`), `AppEvent`, `WindowRegistry`,
  `open_or_focus`, `focus_window`, `system_theme_mode`, `theme::{install,
  theme_config, palette, metrics}`, `windows::{open_main_window,
  open_apps_window, open_preferences_window}`, the three window views.
- Binary `licoman`: `--help`, `--version`; demo env `LICOMAN_DEMO` (now
  `palette|apps|prefs|light|light-apps|failed|distro|desktop|dirty|empty`),
  `LICOMAN_LOG`.

## 3. Decisions and deviations

Recorded in `docs/architecture/decisions.md` (D14–D24); Phase 01 records
D1–D13 unchanged. Headlines: D14 prototype/migration; D15 pins unchanged;
D16 three crates, not five; D17 provider shape, no watch; D18 synchronous
store + adapter-owned visible interval; D19 load-verify/reload for conflicts
instead of polling; D20 validation at apply (matches the diagram's
Validating step); D21 preferences physically separate from the registry;
D22 hand-rolled CLI + logger, no new dependencies; D23 window behavior
modeled in core; D24 unavailable rows show "Unavailable" instead of a
remembered sample value (one intentional visual deviation from Phase 01 —
fail-closed correctness).

## 4. Tests/commands run, with real outcomes

All commands from the repository root; none skipped. Final state:

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | **pass** |
| `cargo test --workspace` | **pass — 59/59** (core 50, ui theme 5, cli 4), 0 failed, 0 ignored |
| `cargo clippy --workspace --all-targets -- -D warnings` | **pass** — 0 warnings (after fixing 6 lints: collapsible-lets, entry-API, never-loops) |
| `cargo build --workspace` | **pass**, 0 warnings |
| `cargo build --workspace --release` | **pass** |

Test coverage mapped to the plan's required list:

1. **Core setting transitions** (`crates/core/src/model_tests.rs`): load→
   edit→discard returns to stored value; load→edit→apply reaches Applied
   with **Confirmed readback** then acknowledges to Idle; invalid value →
   Failed with user-safe validation message + untouched stored value;
   mock write failure → Failed + Retry/Revert; conflict → Reload reconciles
   to the source value; readback **mismatch** → ExternalConflict, not
   success; immediate-mode synchronous apply and its failure path; edit
   guards (read-only, conflicting rows refuse edits).
2. **Stable IDs and search index across selection changes**
   (`search.rs` tests + `two_window_views_observe_the_same_effective_data`):
   hits carry the same stable ids regardless of registration order/selection;
   an edit made through the command path is visible through an independent
   access path (search hit → effective value).
3. **Single-instance/focus + independent navigation histories**
   (`window_sessions.rs` tests): one instance per role, focus-existing on
   repeat, roles independent, per-role histories, duplicate-collapse; plus
   `window_selections_are_independent` at store level. The gpui layer
   delegates decisions to this model and corrects stale handles.
4. **Preferences missing/corrupt/invalid/valid; Dark default; Light choice;
   System fallback; persistence** — 10 `prefs.rs` unit tests (carried from
   Phase 01, all passing) + `theme_preference_is_an_immediate_licoman_only_
   write` + **runtime restart verification on the release binary** (§5).
5. **Theme propagation** — `Theme::change` refreshes every window
   (mechanism unchanged from verified Phase 01); both windows captured in
   both themes this phase (`p02-main-window-{dark,light}.png`,
   `p02-apps-window-{dark,light}.png`); keyboard-focus-no-stale-focus
   remains a human gate (unchanged finding from Phase 01 A5).
6. **`--help`/`--version` + no modules gracefully** — 4 `cli.rs` tests;
   runtime: `./target/release/licoman --version` → `licoman 0.1.0`;
   `LICOMAN_DEMO=empty` capture + `empty_registry_is_usable...` test (store
   usable, search empty, Apply no-ops).
7. **Cargo commands documented in `00_START_HERE.md`** — yes (fmt/test/
   clippy/release + narrower variants), the file exists in the repo root.

The first CI run is pending the user's push (no pushes by this session);
the workflow's system-package list was taken verbatim from upstream
gpui-kit's Linux install script, so it is grounded, not guessed.

## 5. Visual captures and manual verification instructions

All captures on the real session (Hyprland/Wayland, `grim`, window floated,
resized and focused deterministically via `hyprctl`), using the documented
`LICOMAN_DEMO` hooks. **Debug binary** captures (11 files, `docs/screenshots/`):

| File | Shows |
|---|---|
| `p02-main-window-dark.png` | Main window, Dark default, all core-driven rows (dropdown, read-only+note, Unavailable, text, switch, slider) |
| `p02-main-window-light.png` | Same in Light |
| `p02-apps-window-dark.png` / `-light.png` | Applications window both themes (catalog from core, support pills, tabs, sample controls) |
| `p02-desktop-conflict-dark.png` | Desktop scope: **external conflict** row ("Changed outside Licoman" + Reload/Compare), restart-required hint on key-repeat, stored value #70D0A9 shown during conflict |
| `p02-dirty-applybar-dark.png` | Seeded staged edit → "1 change pending" apply bar |
| `p02-failed-state-dark.png` | Failed row: reason + Retry/Revert |
| `p02-empty-state-dark.png` | No providers registered → explicit unavailable state |
| `p02-search-palette-dark.png` | Global palette overlay with empty-state hint |
| `p02-preferences-window-dark.png` | Preferences: theme radios, scope copy, About |
| `p02-release-light-persisted.png` | **Release binary** launched with persisted `theme = "light"` → canvas pixel-sampled `srgb(244,246,250)` = `#F4F6FA` (Light restored across restart) |

Release-binary runtime checks (also logged): `--version` output; headless
run → structured `level=error` record + "no graphical session detected"
diagnostic, exit code 1.

Reproduce:

```sh
cargo run -p licoman                          # main window, dark default
LICOMAN_DEMO=desktop cargo run -p licoman     # conflict + restart hint
LICOMAN_DEMO=dirty  cargo run -p licoman      # staged edit + apply bar
LICOMAN_DEMO=empty  cargo run -p licoman      # no providers
LICOMAN_LOG=info    cargo run -p licoman      # structured logs on stderr
```

Human-only checks unchanged from Phase 01 (no input injection available;
user deprioritized the injector): the keyboard-only walkthrough
(Tab order, Ctrl+K→type→Enter, Esc focus return), live mid-run System-mode
toggle (would require changing the desktop color scheme — forbidden).

## 6. Platform/environment matrix actually tested

| Dimension | Value |
|---|---|
| OS | Arch Linux (kernel 7.2.11-arch1-1) |
| Session | Hyprland (Wayland), `WAYLAND_DISPLAY=wayland-1` (single FALLBACK output 1920×1080 @ scale 1) |
| Rendering | Vulkan (mesa/RADV), gpui blade backend |
| Fonts | system fontconfig (no bundling) |
| Rust | 1.99.1 stable, edition 2024 |
| Headless | verified: readable diagnostic, exit 1 (release binary) |
| X11 | **not tested** (no X11 session available) — a Wayland capture is not proof of X11 support |
| Other DEs | not tested (single-session machine) |

## 7. Security and data-loss assessment

- **No real configuration is read or written.** The only file the
  application can create or modify is
  `$XDG_CONFIG_HOME/licoman/preferences.toml` (fallback
  `$HOME/.config/licoman/…`), written atomically only after a user theme
  change, validated on load. Provider `write()` calls in this phase reach
  the **mock provider only** and mutate in-memory state; there is no code
  path from the UI to any external file, daemon, or process.
- The UI layer literally cannot open config files: `licoman-ui` has no
  filesystem APIs of its own, and `licoman-core` performs exactly two kinds
  of I/O — the preferences file and (via the mock provider, which performs
  none) nothing else. The release binary was additionally verified to
  refuse startup without a display rather than improvising.
- Logs carry setting ids, counts, and error kinds — never values, file
  contents, or credentials (`licoman_core::log` module rule +
  CONTRIBUTING.md).
- No privileged operations, no subprocesses, no network, no telemetry, no
  daemon, no idle polling. CI installs only the upstream-documented build
  packages.

## 8. Remaining work and acceptance gates

| Acceptance criterion (02_FOUNDATION.md) | Status |
|---|---|
| Actual main + Applications windows run using GPUI Kit, rendering the approved Licoman design | **pass** — both windows captured this phase (§5) on the new crate structure, tokens identical to Phase 01 |
| Core is UI-independent and has mock-verified typed state transitions | **pass** — `licoman-core` has no gpui dependency (structural); 50 headless tests incl. the full transition set |
| App never writes to external application, DE or system config files | **pass** — only Licoman's own preferences.toml; UI has no I/O path; mock writes are in-memory |
| Dark fresh-install default plus persistent Light/Dark/System | **pass** — prefs unit tests + release-binary light-persistence capture; Dark default unchanged and re-verified in dark captures (no preferences file used for default runs) |
| No bundled daemon, browser or JavaScript extension runtime | **pass** — dependency set unchanged (gpui-kit, serde, toml); no daemons/processes spawned |
| No unexplained high idle CPU activity or unmanaged timer loops | **pass** — zero timers at idle; the only two timers run during an apply (700 ms) and its acknowledgment (1.8 s) |
| Unsupported environments show user-readable diagnostics | **pass** — no-display launch diagnostic (exit 1); no-provider empty state; unavailable/permission rows with reasons |
| Existing prototype remains available as a regression reference | **pass (adapted)** — no external prototype exists (D1); the Phase 01 implementation/IDs/screenshots are preserved via history-keeping moves and unchanged fixtures (D14) |
| `docs/architecture/` and `docs/handoffs/PHASE_02_REPORT.md` explain decisions and test results | **pass** — this file + architecture README + D14–D24 |

Known gaps / carried-over gates (all pre-existing, none introduced):
- Keyboard-only walkthrough remains a human gate (no input-injection tool;
  Phase 01 A5, user-deprioritized).
- X11 untested (no session available; plan acknowledges this explicitly).
- CI unexercised until the next push (pushes are out of scope for the
  automated session); the workflow mirrors upstream gpui-kit's package list.
- The header-search → palette handoff focuses the palette input from
  `render()`; pointer/keyboard behavior of this path should get one manual
  glance during review (programmatic focus in render is the only spot
  where gpui-kit focus timing is not fully verified).

**Stop condition:** this is the end of Phase 02. Per the plan — "Stop after
this phase." Next session starts Phase 03 (`03_ENVIRONMENT_DISCOVERY.md`)
from this report.

## 9. Reverting this phase

No user configuration is at risk: the only file the app may create is
`$XDG_CONFIG_HOME/licoman/preferences.toml` — delete the `licoman/` config
directory to return to a pristine state. During verification this session
used a throwaway `XDG_CONFIG_HOME` (`/tmp/licoman-xdg`); the real config was
touched only by normal theme-save paths, and the default runs never created
a preferences file.

To revert the repository to the pre-Phase-02 state:

```sh
cd /home/baris/Work/licoman
git checkout -- . && git clean -fd .github 00_START_HERE.md CONTRIBUTING.md crates docs/architecture docs/screenshots/p02-*
# keeps Phase 01 (commit 6852c1d) exactly as pushed
```

Nothing outside the workspace was modified; no commits or pushes were made.
