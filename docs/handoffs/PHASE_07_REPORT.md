# Phase 07 Report — First Live Settings Integrations (Controlled Vertical Slices)

**Date:** 2026-11-03
**Workspace:** `/home/baris/Work/licoman` (`main`, Phase 06 commit + this
phase's uncommitted changes — nothing was committed or pushed by the
automated session, per the execution contract; the tree is ready for
review).
**Executor:** automated session (ZCode), one phase per session as required.
**Plan source:** `07_FIRST_REAL_INTEGRATIONS.md` (author's document pack).
All nine section headings of the required handoff format are below.

---

## 1. Goal and actual scope implemented

**Goal (from the plan):** prove Licoman can safely read and change a few
**real** settings via three distinct mechanisms — Alacritty (TOML), GNOME
GSettings, and PowerProfiles over D-Bus — using the registry, typed core,
design system and safety pipeline established earlier, with live writes
requiring explicit per-test user approval and never running in `cargo
test`, CI, startup or discovery.

All three slices are implemented as **live write paths behind the same
Phase 06 engine gates** (one engine everywhere — D49), plus the optional
Hyprland read-only diagnostics module (implemented, live-verified):

- **A — Alacritty:** version-validated exposure (D50). The file backend
  detects the installed Alacritty with a bounded, direct
  `alacritty --version` (no shell); every option requires the TOML era
  (≥ 0.13.0 per the upstream changelog); an undetectable version disables
  writes (fail-closed) while reads stay available; legacy YAML-only setups
  are a distinct honest unavailable state. The manifest was corrected
  against the official docs: `cursor_blink` bound to the real
  `cursor.style.blinking` enum (Never/Off/On/Always, default Off — was a
  mock boolean), `cursor.unfocused_hollow` key fixed (was wrongly
  `cursor.style.unfocused_hollow`), `bell.animation` restricted to the
  era-stable easing set (Ease/EaseOutQuad/Linear, default Linear — the
  previous list contained values Alacritty does not know), defaults
  aligned (padding 0, font 11.25, dimensions 0), and restart requirements
  declared where the docs say changes need a restart (dimensions,
  startup_mode, live_config_reload). **On this machine Alacritty is not
  installed**, so the live state is the honest "requires alacritty.desktop —
  not installed"; the full write pipeline (conflict checks, backups,
  readback, plus the new xattr gate) is machine-tested on fixture roots.
- **B — GNOME/GSettings:** a native write backend (D51) over the `gio`
  crate — typed `set_value` coerced to the **schema's own variant type**
  and range-checked against the schema before any call, `reset` using the
  platform's actual reset semantics, verification reading back through the
  same native transport after an explicit `g_settings_sync()`. A fail-closed
  **desktop gate** (fed from discovery) refuses writes outside a GNOME
  session even when schemas are installed — on this Hyprland machine the
  module is inapplicable *and* the gate is closed. External changes are
  caught twice: native `changed` signals (a dedicated gio watcher thread
  feeding the established debounced refresh) and the existing dconf-db
  file watcher; pending edits survive and conflict at apply time. Row
  reads keep the self-contained GVDB parser (fixture tests stay
  desktop-independent).
- **C — PowerProfiles:** the real zbus write client (D52) — typed
  `Properties.Set` on the passively probed endpoint (current and legacy
  names), readback-verified, persistence reported **`Unknown`** because
  the interface documents no persistence guarantee. The reader now also
  surfaces `PerformanceDegraded` (a new read-only row) and the
  profile-holds count, and `Profiles` decoding was fixed for the current
  daemon's `aa{sv}` shape (dicts with a `Profile` entry) while keeping the
  legacy string-array shape. **The service is not running on this
  machine** → the UI shows the truthful per-row "service not found on the
  session or system bus" states (screenshot); the write path is
  fault-tested against the typed fake (denial / timeout /
  change-after-apply are all honest non-successes).
- **Optional Hyprland diagnostics** (plan section implemented): a
  read-only provider over the documented IPC socket — compositor version,
  active workspace, monitors — each row labeled "Runtime only — read live
  from the compositor's documented IPC; never written and not persisted
  anywhere", in its own **Compositor** category. Live-verified against
  Hyprland 0.56.2 and **0.57.0** (the compositor updated mid-phase); the
  IPC client sends single bare commands (0.56 rejects newline-terminated
  query requests with "unknown request").

### The controlled live verification (user-approved)

Per the plan's protocol: all automated tests ran on fixtures/fakes first;
a written operation list (`docs/live-testing/PHASE_07_LIVE_CHECKLIST.md`
§4) was presented and the user **approved exactly L1** (plus screenshots).
L1 = set `org.gnome.desktop.interface clock-show-weekday` → `true`, read
back, reset, read back — through Licoman's real native stack against a
**disposable** dconf database.

- **First attempt — incident (reported honestly):** the disposable
  isolation used only a process-level `XDG_CONFIG_HOME` override; dconf
  writes are routed by the **session bus's dconf-service**, whose database
  path comes from its own environment, so the write reached the **real**
  user database. The harness panicked at its own CLI cross-check (the CLI
  read the untouched temp db) and never reached its reset step. **Recovery
  was immediate and verified:** native `gsettings reset` (the platform's
  own semantics), `gsettings get` → `false`, the key absent from
  `dconf dump`, no other key touched (the user's real overrides were never
  involved). Recorded in the threat model and below (§7).
- **Isolation fixed:** the run now executes under a **private session
  bus** (`dbus-run-session`) with the harness pre-spawning
  `dconf-service` with the disposable environment (a private bus alone was
  not enough — dbus activation routed the service through the systemd user
  instance, whose environment carries the real config home).
- **Approved L1 re-run — passed end to end:** engine set → `Applied`
  (engine-verified), `gsettings get` → `true` immediately, engine reset →
  `Applied`, `gsettings get` → `false`, `dconf dump` of the disposable db
  shows no override, and the **real database's SHA-256 was byte-identical
  before and after** (`3e1f63c2…`). The engine-level lesson became code:
  `g_settings_sync()` before any readback (a client-queued dconf write
  would otherwise "verify" its own local view).

**Not approved, not executed:** L2 (real database write), L3 (Alacritty
install + live file write), L4 (power-profiles-daemon install). No
privileged operations, no package actions by Licoman, no Hyprland config
writes, no dconf internal editing.

## 2. Changed/added files and public interfaces

```text
crates/core/src/read/alacritty.rs      NEW: version detection + per-option
                    validation + era-stable enum values (D50); AlacrittySupport
crates/core/src/write/gsettings_write.rs NEW: GSettingsWriteBackend,
                    GSettingsDesktopGate, NativeGSettings trait,
                    GioNativeGSettings (schema-typed coercion, range check,
                    reset semantics, sync-before-readback), GsClientError
crates/core/src/hyprland.rs            NEW: HyprlandDiagnostics provider
                    (read-only, documented IPC, Runtime origin, own
                    "Compositor" category)
crates/core/tests/live_p07.rs          NEW: the #[ignore]d + env-gated live
                    verification harness (never runs in CI/cargo test)
crates/core/src/read/file.rs       + AlacrittySupport wiring (from_env detect,
                    for_dirs assumes TOML era for fixtures), write_support(),
                    legacy-YAML candidate detection, version line in details
crates/core/src/read/mod.rs        + alacritty module
crates/core/src/read/router.rs     + write_support() (family-level writability)
crates/core/src/read/gsettings.rs  + spawn_native_change_watcher (gio signals
                    on a dedicated thread with its own main context)
crates/core/src/read/dbus.rs       + ZBus set_property (typed Properties.Set),
                    aa{sv} Profiles decoding, PerformanceDegraded read,
                    holds/degraded diagnostics on the profile read,
                    borrowed-triple inherent property/set_property
crates/core/src/write/file_write.rs + XattrOps trait + RealXattrOps (D53:
                    collect before staging, restore onto the staged file,
                    any failure refuses pre-rename), write_support gate at
                    the top of apply()
crates/core/src/write/engine.rs    + gsettings backend field/attach/dispatch
crates/core/src/write/mod.rs       + gsettings_write module + backend re-exports
crates/core/src/write/service_write.rs + ServiceWriteClient for the real
                    zbus client (honest Unknown persistence)
crates/core/src/write/phase07_matrix_tests.rs NEW (7 scenarios)
crates/core/src/write/write_matrix_tests.rs   gsettings gate test updated to
                    the Phase 07 semantics (no backend → honest refusal)
crates/core/src/modules/provider.rs + ModuleProvider::live() (D49), with_
                    write_engine, gsettings gate refresh in set_environment,
                    per-family writable flips (version gate / dbus presence /
                    desktop gate), GSettings reset support, native-transport
                    verification for gsettings writes, gsettings_watch_keys()
crates/core/src/modules/registry.rs fixture_writes → write_controls (rename,
                    semantics: presentation flag for engine-attached providers)
crates/core/src/provider.rs        Provider: Send + Sync supertraits (shared
                    handles by design, mirrors SettingsReadBackend)
crates/core/src/mock.rs            RefCell → Mutex (Send/Sync)
crates/core/src/prefs.rs           + data_dir() ($XDG_DATA_HOME/licoman)
crates/ui/src/gsettings_watch.rs   NEW: native-change → debounce → futures
                    channel → refresh (same shape as settings_watch)
crates/ui/src/app_state.rs         + gsettings watcher lifecycle, extra
                    readers (per-id read routing — hyprland ids served by
                    their provider), failed background reads logged (id +
                    error kind only)
apps/licoman/src/main.rs           live provider construction (ModuleProvider::
                    live), Hyprland provider registration (live runs only)
                    + extra-reader sharing, LICOMAN_CAPTURE_NAV navigation
                    hook (live captures; navigation only)
modules/builtin/app-alacritty.toml corrected bindings/defaults/restarts (§1)
modules/builtin/desktop-gnome-interface.toml  description + level partial
modules/builtin/shared-power.toml  + performance_degraded row, honest profile
                    description
tests/tools/capture-p07.sh         NEW live-capture harness (Hyprland 0.57
                    compatible; see §5)
docs/live-testing/PHASE_07_LIVE_CHECKLIST.md NEW (required deliverable)
docs/architecture/integrations.md  NEW (required deliverable: supported
                    combinations and limits)
docs/architecture/write-engine.md  + Phase 07 section, limitations updated
docs/architecture/backends.md      + write-capable backends summary
docs/security/threat-model.md      + Phase 07 state + incident record
docs/architecture/decisions.md     D49–D54
00_START_HERE.md, README.md        status banners
docs/handoffs/PHASE_07_REPORT.md   this file
Cargo.toml / crates/core/Cargo.toml + gio 0.18, xattr 1 (core)
.github/workflows/ci.yml           core + workspace jobs install
                    libglib2.0-dev (the native GSettings backend needs
                    libgio/libglib at build time; present on every desktop
                    the app targets)
```

Key public interfaces:

- `licoman_core::read::alacritty`: `AlacrittySupport`, `AppVersion`,
  `detect()`, `parse_version_output()`, `option_min_version()`,
  `option_values()`, `yaml_candidates()`, `TOML_ERA_MIN`.
- `licoman_core::read::file::FileReadBackend`: `with_alacritty_support()`,
  `alacritty_support()`, `write_support()`.
- `licoman_core::read::router::BackendRouter`: `write_support()`.
- `licoman_core::read::gsettings`: `spawn_native_change_watcher()`,
  `NativeChangeWatch`.
- `licoman_core::read::dbus::ZBusPowerProfilesClient`: inherent
  `property`/`set_property` (borrowed triples); `ServiceWriteClient` impl.
- `licoman_core::write`: `GSettingsDesktopGate`, `GSettingsWriteBackend`
  (`native`, `with_native`, `gate`, `permitted`, `native_read`),
  `NativeGSettings`, `GsClientError`, `FileWriteBackend::with_xattr_ops`,
  `XattrOps`/`RealXattrOps`, `WriteEngine::with_gsettings`/`gsettings`.
- `licoman_core::modules::ModuleProvider`: `live()`, `with_write_engine()`,
  `gsettings_watch_keys()`.
- `licoman_core::prefs::data_dir()`; `licoman_core::hyprland::
  HyprlandDiagnostics` (`new`, `with_socket`, `Clone`).
- `licoman_ui::gsettings_watch::start()`; `AppState::add_extra_reader()`.
- Binary: `LICOMAN_CAPTURE_NAV="Scope/Category"` (live-capture navigation;
  documented in the capture script).

## 3. Decisions and deviations

Recorded in `docs/architecture/decisions.md` (D49–D54): **D49** one engine
with identical gates for fixture and live providers; **D50** Alacritty
exposure version-validated, fail-closed on unknown versions;
**D51** GSettings writes via the native API behind a fail-closed desktop
gate; **D52** PowerProfiles typed property writes with honest `Unknown`
persistence; **D53** xattr/ACL preserve-or-refuse (closing the Phase 06
gate); **D54** controlled live verification on disposable roots.

Deviations / judgment calls (all disclosed):

- **New dependencies in core:** `gio` 0.18 (the native GSettings API the
  plan demands — no shell tool, no dconf internals) and `xattr` 1 (the
  Phase 06 preserve-or-refuse gate). `gio` links libgio/libglib: the CI
  core job installs `libglib2.0-dev`, and the runtime requirement
  (glib ≥ 2.66-era, universal on desktops) is documented in
  `integrations.md`.
- **D52 persistence label is `Unknown`, not `RuntimeOnly`** (the decision
  text was amended accordingly): the interface documentation promises
  nothing about persistence either way, and the plan forbids promising
  persistence without a guarantee. "Runtime only — a restart reverts it"
  would have been a negative claim we cannot cite either.
- **The optional Hyprland module is a code-defined provider, not a
  manifest module:** the manifest grammar's closed backend allowlist has
  no IPC kind, and adding one for a diagnostics-only feature would widen
  the module contract; a read-only provider with `Control::ReadOnly` rows
  and no write path is the narrower surface. Its rows live in their own
  "Compositor" category (separate from the unavailable Hyprland
  configuration module's rows).
- **Alacritty config creation is deferred** (the plan's "If config is
  absent, creation requires explicit user intent…"): the app is not
  installed on this machine, so no creation flow could be honestly
  verified; the absent state is the honest primary handling. Documented as
  remaining work.
- **Live verification automation drives the store → engine path** rather
  than GUI clicks (no input injector exists on this machine — a standing
  limitation since Phase 01); the UI's interactive dialogs remain a human
  gate. The harness is `#[ignore]`d **and** env-gated, so `cargo test` and
  CI can never reach it.
- **The desktop gate for the live test was opened by the fixture GNOME
  snapshot** (`fedora-gnome`) while the real schema/bus/write stack ran
  against the disposable root — the controlled declaration documented in
  the checklist; the alternative (faking the session) was rejected as
  less honest.

**Bugs found and fixed during verification** (all caught by tests or the
live runs, recorded honestly):

1. **Phase 05 latent read bug — `Profiles` shape:** the current daemon
   returns `aa{sv}` (dicts with a `Profile` entry), not `as`; the old
   parser would have failed against a real current daemon. Both shapes now
   decode; unit-tested.
2. **Read-pass routing:** background reads sent every id to the module
   provider, so the Hyprland provider's rows could never be read live
   (`reader_for` per-id routing added; the failure was invisible until
   failed reads were logged — a redacted log line was added too).
3. **Hyprland IPC trailing newline:** newline-terminated query requests
   are answered "unknown request" on 0.56.2; bare commands work on 0.56
   and 0.57 (verified against both).
4. **Socket existence check:** `is_file()` is false for sockets; the
   provider's socket probe used `exists()`.
5. **Capture navigation had no repaint:** programmatic navigation commands
   report `settings_changed: false`; the capture hook now notifies the
   windows explicitly (interactive clicks always did this via listeners).
6. **GSettings readback self-verification:** `g_settings_set_value` queues
   at the dconf client; without `g_settings_sync()` the engine's readback
   verified the client-local view. The backend now syncs before
   verifying — service-authoritative readbacks.
7. **The isolation incident itself** (§1/§7): process-env isolation is
   insufficient for dconf writes; private session bus + self-started
   `dconf-service` is the fixed procedure, documented in the checklist.
8. **Provider trait threading:** `Provider` gained `Send + Sync`
   supertraits (implementations are shared handles by design, mirroring
   `SettingsReadBackend`); `MockProvider`'s `RefCell` became a `Mutex`.

## 4. Tests/commands run, with real outcomes

All commands from the repository root; none skipped. Final state:

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | **pass** |
| `cargo test --workspace` | **pass — 258/258** (core 247 incl. the 7-scenario Phase 07 matrix + gsettings/alacritty/hyprland suites, cli 6, ui 5) + **1 ignored** (the live harness, by design) |
| `cargo clippy --workspace --all-targets -- -D warnings` | **pass — 0 warnings** |
| `cargo build --workspace --release` | **pass** |
| `./target/release/licoman --version` | `licoman 0.1.0` |
| `./target/release/licoman module validate modules/builtin/` | 15/15 valid |
| Live binary smoke runs (read-only, 6–14 s) | discovery, real reads (gsettings origin `User`/`Default`), gate `open=false` on Hyprland, Hyprland diagnostics rows read, **zero write events** |
| `LICOMAN_DEMO=writes` smoke (7 s) | Phase 06 states unchanged on the new engine (reads published, both demo edits staged) |
| **Approved L1 live operation** (final run) | **pass** — engine `Applied` both ways, `gsettings get` `true`/`false` verified immediately, disposable db dump clean after reset, real db SHA-256 byte-identical (`3e1f63c2…`) |

Phase 07 matrix coverage (new `write::phase07_matrix_tests` + suites):

| Scenario | Test | Result |
|---|---|---|
| Unknown version disables writes, reads stay available | `unknown_version_disables_writes_while_reads_stay_available` | pass |
| Legacy YAML-only setup is a distinct state | `legacy_yaml_only_configuration_is_a_distinct_unavailable_state` | pass |
| TOML-era version writes normally, file preserved | `toml_era_version_writes_normally_and_preserves_the_file` | pass |
| User xattrs survive a write | `user_xattrs_survive_a_write` | pass |
| Unreadable xattrs refuse up front | `unreadable_xattrs_refuse_the_write_up_front` | pass |
| Unrestorable xattrs refuse, no debris | `unrestorable_xattrs_refuse_the_write_and_leave_no_debris` | pass |
| Desktop gate drives writability; store-level native apply + native reset support | `gsettings_writability_follows_the_desktop_gate_and_the_store_applies_natively` | pass |
| Native round trip / gate closed+unknown / external change conflict / type mismatch refusal / absent schema+key / non-gsettings target | `write::gsettings_write` suite (6 tests) | pass |
| Version parsing/gates/support states | `read::alacritty` suite (4 tests) | pass |
| Hyprland IPC read/parse/refusals/labels | `hyprland` suite (4 tests, fixture socket) | pass |

**CI impact:** the core job now installs `libglib2.0-dev pkg-config` (the
native GSettings backend); the workspace job adds `libglib2.0-dev` to its
existing apt list. No test touches a real dconf database; the live harness
is ignored by default. **CI outcome for this phase is unverified until
push** (no `gh` CLI; check the Actions tab after the user pushes and
record the result in the Phase 08 report if it failed).

**Carried over, now closed:** the Phase 06 CI outcome was verified at the
start of this session — run `41241062646` for `94eb2b5` completed
**success**.

## 5. Visual captures and manual verification instructions

Captured live (no demo fixtures) with `tests/tools/capture-p07.sh`,
desktop free, each image verified against its expected state:

| File | Shows (verified) |
|---|---|
| `p07-live-hyprland.png` | Desktop → **Compositor** category: the three read-only runtime rows with live values (Active workspace 1, Monitors DP-5, Compositor version 0.57.0), each with the "Runtime only" note and "Live service state" origin line |
| `p07-live-power.png` | Shared → Power with the service absent: **Unavailable** states for Power profile / Available profiles / **Performance degraded** (the new row), each with the passive-probing explanation, alongside the declared mock rows |

Reproduce:

```sh
tests/tools/capture-p07.sh all
# or run the live app with navigation only:
LICOMAN_CAPTURE_NAV="Desktop/Compositor" ./target/release/licoman
```

The Phase 06 demo captures (`p06-*.png`) remain valid — the writes demo
states are unchanged on the new engine (smoke-verified).

Human-only checks (no input injector exists): the confirmation dialog,
Undo, and reset-disclosure flows need hands on the desktop; the underlying
store transitions are asserted by the core tests, and the live L1 run
exercised the apply and reset paths end to end headlessly.

## 6. Platform/environment matrix actually tested

| Dimension | Value |
|---|---|
| OS | Arch Linux (kernel 7.2.11-arch1-1), host context |
| Compositor | Hyprland 0.56.2 → **0.57.0** (updated mid-phase; both verified for the diagnostics IPC) |
| Session | Wayland; demos/tests headless; live runs on the real session |
| GSettings | real schemas + real dconf user db; writes verified against a **disposable** database under a private session bus; real db read-only throughout (fingerprint-checked) |
| D-Bus | system bus live (upowerd present); power-profiles service absent → honest states |
| Alacritty | not installed → honest absent state; fixture-root pipeline fully tested |
| File writes | fixture/temp roots + the disposable dconf root only; the only real-filesystem writes remain Licoman's own preferences file |
| Rendering | Vulkan (mesa/RADV), gpui blade backend |
| Rust | 1.99.1 stable, edition 2024; **new core dependencies: `gio 0.18`, `xattr 1`** |
| CI | core job gains `libglib2.0-dev pkg-config`; workspace job adds `libglib2.0-dev`; no test touches real settings |

## 7. Security and data-loss assessment

- **The incident (disclosed, contained, fixed):** the first L1 attempt
  wrote one key (`clock-show-weekday=true`) to the real user dconf
  database because process-env isolation does not bound dconf writes
  (service-side path resolution). Recovery: native `gsettings reset`
  immediately after detection; verified by `gsettings get` → `false`,
  absence from `dconf dump`, and the unaffected real overrides. Root
  cause + fix recorded in three places (checklist, threat model, here).
  The approved re-run was fingerprint-clean end to end.
- **No other real state was touched:** Alacritty absent (nothing to
  write), power-profiles service absent (nothing to write), the live
  binary's only writes remain Licoman's own preferences file, and the
  live smoke runs logged zero write events.
- **The write paths fail closed:** version-unknown Alacritty, non-GNOME
  sessions, missing schemas, unrestorable xattrs, foreign-owned files,
  fingerprint/value conflicts — each refuses before touching anything.
- **Least privilege unchanged:** deny-by-default broker; no sudo, no
  password collection, no root path; the GSettings path has no command
  execution surface (typed libgio calls only).
- **Privacy unchanged:** logs and operation records carry ids, redacted
  targets, outcomes, timings — never values (asserted by the planted-
  secret test, still green).

## 8. Remaining work and acceptance gates

| Acceptance criterion (07_FIRST_REAL_INTEGRATIONS.md) | Status |
|---|---|
| Alacritty, GSettings and PowerProfiles pathways show correct real reads or explicit unavailable states | **pass** — live screenshots + smoke runs; Alacritty/PowerProfiles honestly absent on this machine |
| At least two backend types perform safe user-approved writes in a suitable test environment | **pass** — GSettings: live user-approved round trip on the disposable root (real native stack); file: the full engine pipeline user-approved on fixture roots (Phase 06 demos + Phase 07 gates), live file write not executed (no Alacritty — user declined L3); PowerProfiles: typed-fake verification, live write not possible (service absent) |
| No live write runs in CI, `cargo test`, application startup, discovery or without explicit user Apply | **pass** — the live harness is `#[ignore]`d + env-gated; live engines attach only in the binary; zero write events in smoke runs; L1 executed only after the user's explicit approval of that exact operation |
| Staged/apply/reset/conflict UX accurately communicates backend outcomes | **pass** — single `publish_apply` mapper unchanged; GSettings reset discloses "removes your override" and uses the platform's real reset; persistence labels honest (`Unknown` for PowerProfiles) |
| No arbitrary script execution, host-root GUI or distribution package mutations | **pass** — the only spawned processes are the documented `alacritty --version` probe (direct exec, bounded), `dconf-service` (in the disposable private session, harness-only) and verification CLIs (`gsettings get`/`dconf dump` — reads only) |
| Tests and documentation cover precise limits rather than claiming "all Linux supported" | **pass** — `docs/architecture/integrations.md` is per-integration precise |

Known gaps / carried-over gates (all disclosed):

- **Alacritty config-creation flow** (explicit user intent for an absent
  config) is deferred — the app is not installed here; documented as
  remaining work (§3).
- **L2/L3/L4** were not approved and not executed; the checklist stays
  valid for a future session.
- **Interactive dialog walkthroughs** remain human gates (no input
  injector).
- **CI outcome for this phase is unverified until push** (§4).
- PowerProfiles **holds** are surfaced as a count only; the Hold()/Release
  client API is out of scope.

**Stop condition:** this is the end of Phase 07. Per the plan — "Stop
here. Review safety and UX outcomes before adding more provider
families." The review inputs are this report (including the incident),
`docs/live-testing/PHASE_07_LIVE_CHECKLIST.md`, `docs/architecture/
integrations.md` and `docs/security/threat-model.md`.

## 9. Reverting this phase

No user configuration is at risk: the real dconf database is byte-
identical to its pre-phase fingerprint (`3e1f63c2…`, verified after the
approved run; the incident's single key was restored by native reset and
verified). Backups under `$XDG_DATA_HOME/licoman/backups` may contain
copies from any *user-driven* live applies; deleting that directory
removes them.

To revert the repository to the pre-Phase-07 state:

```sh
cd /home/baris/Work/licoman
git checkout -- . && git clean -fd \
  crates/core/src/read/alacritty.rs \
  crates/core/src/write/gsettings_write.rs \
  crates/core/src/write/phase07_matrix_tests.rs \
  crates/core/src/hyprland.rs \
  crates/core/tests/live_p07.rs \
  crates/ui/src/gsettings_watch.rs \
  docs/live-testing docs/architecture/integrations.md \
  tests/tools/capture-p07.sh docs/screenshots/p07-*.png
# keeps Phase 06 (commit 94eb2b5) exactly as pushed
```

The two new dependencies drop out with `Cargo.toml`/lockfile via the
checkout. Nothing outside the workspace (and the verified-identical dconf
db) was modified; no commits or pushes were made.
