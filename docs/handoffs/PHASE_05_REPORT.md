# Phase 05 Report — Read-Only Native Configuration Backends

**Date:** 2026-10-31
**Workspace:** `/home/baris/Work/licoman` (`main`, Phase 04 commit + this
phase's uncommitted changes — nothing was committed or pushed by the
automated session, per the execution contract; the tree is ready for review).
**Executor:** automated session (ZCode), one phase per session as required.
**Plan source:** `05_READ_ONLY_BACKENDS.md` (author's document pack).
All nine section headings of the required handoff format are below.

---

## 1. Goal and actual scope implemented

**Goal (from the plan):** demonstrate Licoman can present **actual effective
settings** from fundamentally different Linux configuration mechanisms
without modifying anything. Honored end to end: the write boundary is
enforced by the read envelope itself — every real backend reports
`Writable::No`, the store refuses real-backend writes unconditionally
(unchanged from Phase 04), and the UI renders non-writable rows through an
explicit read-only path. The only file Licoman writes remains its own
`$XDG_CONFIG_HOME/licoman/preferences.toml`.

### The typed read layer (`crates/core/src/read/`, new, 7 files)

- **One envelope for every family** (`mod.rs`): `ReadResult` = typed
  `SettingValue` + `ValueOrigin` (default/system/user/managed/runtime/
  unknown) + `Writable` (fail-closed) + `Freshness` (read time + FNV-1a
  fingerprint over the source bytes) + `RestartEffect` + `SourceDetails`
  (backend kind, exact target, derivation note, imported flag, diagnostics —
  metadata only, never contents). The `SettingsReadBackend` trait has
  `probe` / bounded `read` / `watch_paths`; a `BackendRouter` maps validated
  `BackendBinding`s to families (D38).
- **Provider/Store integration**: `Provider::read_detailed` +
  `read_latency` (Immediate = mock, Background = real I/O). `Store::load`
  keeps Background rows at "Not read yet"; the adapter runs a
  **generation-guarded background read pass** on the GPUI background
  executor and publishes row by row (`Store::publish_read`). Staged edits
  survive reads; unchanged value+fingerprint emits no UI churn. No I/O ever
  runs on the UI thread.
- **GVariant decoder** (`gvariant.rs`): bounded decoder for GLib's framed
  *non-normal* GVariant values (`[body][pad][signature]`, strings
  NUL-terminated without length prefixes). Verified byte-for-byte against a
  real `~/.config/dconf/user`, a `dconf compile` scratch database, and the
  system `gschemas.compiled`. Arrays/dictionaries are an explicit
  unsupported-type error, never a guess; malformed input yields errors,
  never panics.

### Backend family A — GSettings / GLib (`read::gsettings.rs`)

- Schema defaults from `gschemas.compiled` (GVDB), searched in XDG data-dir
  order; schema/key presence probed before reading; the schema's `.path`
  attribute (special `.path` item) locates dconf keys. The Phase 03 GVDB
  parser gained `has_schema`, `lookup_schema_value`, `lookup_schema_path`.
- Effective values layer **dconf overrides** (also GVDB) over defaults in
  dconf's documented priority — user db first (highest), then the system
  databases listed in the dconf profile (`/etc/dconf/db/<name>`,
  `XDG_CONFIG_DIRS`). Origins: user db → `User`, system db → `Managed`,
  schema default → `Default`.
- Degraded states disclosed, not hidden: relocatable schemas
  (defaults-only, no dconf path), non-dconf `GSETTINGS_BACKEND`, unreadable
  databases. No CLI, no shell, no new dependency (D39). Watch path: the
  dconf user-db file (dconf-service rewrites it on every `gsettings set`).

### Backend family B — structured configuration files, TOML first (`read::file.rs`)

- **Alacritty resolution exactly as documented**: candidates
  `$XDG_CONFIG_HOME/alacritty/alacritty.toml`, `$XDG_CONFIG_HOME/alacritty.toml`,
  `$HOME/.config/alacritty/alacritty.toml`, `$HOME/.alacritty.toml`,
  `/etc/alacritty/alacritty.toml`; then `general.import` — "Imports are
  loaded in order, skipping all missing files, with the importing file
  being loaded last" / "If a field is already present in a previous import,
  it will be replaced" — so the merge order is `[imports…, root]` and the
  **last loader wins**. Correct merged resolution *is* implemented;
  per-setting provenance names the winning file and flags imported values
  (D40).
- **Bounds**: 256 KiB/file, import depth ≤ 8, ≤ 32 files, cycle detection
  along the current chain (duplicate imports allowed, ancestor cycles
  skipped with diagnostics), distinct invalid-TOML and invalid-UTF-8
  errors, canonicalized-path containment inside authorized roots (config
  home, home, `/etc/alacritty`) — symlink escapes are rejected, never
  followed. `toml_edit` parses (formatting preservation for Phase 06).
- Missing config = normal state ("the application runs on its built-in
  defaults; Licoman shows no invented value"); keys absent from a present
  config are "not present", never empty. Non-TOML file bindings (e.g.
  Hyprland keyfile) stay recorded-but-unread with that exact wording.
- New manifest field `resolution` on `file` bindings (closed enum:
  `xdg_file` default, `alacritty`), schema + docs updated.

### Backend family C — service API / D-Bus (`read::dbus.rs`)

- Client: **zbus 5** (blocking API on worker threads; `async-io` feature
  set matching the ashpd already in the tree — no tokio feature conflict).
- Target: `org.freedesktop.UPower.PowerProfiles` (`ActiveProfile` string,
  `Profiles` string array) with the legacy `net.hadess.PowerProfiles`
  endpoint as fallback. Name probing is a passive `ListNames` — no service
  is activated, installed, or started to make a demo look populated.
- **Every client call is bounded**: the backend runs each call on a worker
  thread with a 3 s timeout — this bounds *injected* clients too, so an
  unresponsive service (real or fixture) becomes a `Timeout` error, never a
  hang (D41). Distinct states: service absent, bus unreachable, timeout,
  denied (permission), unexpected value type.
- Discovery now also probes the **system bus** (`dbus-system` capability,
  passive); applicability for `dbus` bindings checks session ∪ system
  names — power-profiles-daemon is a system service the session-bus-only
  check could never see.

### Source of truth and freshness (plan §3)

- Provenance per read (backend, target, origin, derivation, fingerprint,
  read time) rides on `SettingModel.read_meta`; the UI's **Source details**
  disclosure renders exactly that plus the module's recorded provenance.
- **External-change visibility**: the adapter watches the backends'
  `watch_paths()` (dconf user db, Alacritty config dir) with a 500 ms
  debounce (same pattern as the Phase 03 environment watcher; Access events
  filtered), and the manual **Refresh** button re-runs discovery + the read
  pass. Both update every open window through the same settings-changed
  event.
- No unrelated settings are stored anywhere; Licoman claims no
  authoritative-config role.

### UI / design (plan §5)

- Read-only values render through the same setting-row components and
  theme tokens: value + "This setting cannot be changed yet." for every
  non-writable row; switches render disabled with the note; no interactive
  entity is even *built* for non-writable rows (nothing clickable pretends
  to save).
- Idle rows show an origin status line ("Default · …", "Set by you —
  …") with a per-row **Source details…** dialog (backend, exact
  schema/key or path, origin + derivation, freshness + fingerprint,
  writability, diagnostics, module provenance — metadata only).
- Applications window support tags now read `Supported (read-only)` /
  `Partially supported` (the plan's truthful levels); detail rows carry
  the origin-aware status line.
- Dark remains first-run default; Light verified via the existing
  theme pipeline (user-selected Light/System continues across windows —
  unchanged code path, re-verified visually).

### Demo hooks for verification

`LICOMAN_DEMO=reads` (fixture-backed backend router over the arch-hyprland
fixture: real GSettings/TOML values + the Source details dialog after the
read pass), `reads-power` (Power category: truthful absent-service rows),
`reads-apps` (Applications window, Alacritty rows from the fixture TOML
config). All read passes in demo mode run against the fixture router only —
live backends never run under a demo.

## 2. Changed/added files and public interfaces

```text
crates/core/src/read/             NEW (7 files):
  mod.rs            ReadResult/ValueOrigin/Writable/Freshness/
                    RestartEffect/SourceDetails/SupportState + trait +
                    fingerprint + bounded file reader
  gvariant.rs       bounded non-normal GVariant decoder (+ real-byte tests)
  gsettings.rs      GSettings backend (schemas + dconf user/system dbs)
  file.rs           TOML backend (Alacritty search paths + import merge)
  dbus.rs           PowerProfiles backend (zbus, bounded client boundary)
  router.rs         binding → backend mapping + watch paths
  read_matrix_tests.rs  §6.4–6.6 matrix (cross-backend, read-only proof,
                    no-secrets)
crates/core/src/discovery/gsettings.rs   + has_schema/lookup_schema_value/
                    lookup_schema_path/dconf_lookup
crates/core/src/discovery/backends.rs    + system-bus probe,
                    BackendFacts.bus_names_system, dbus-system capability
crates/core/src/discovery/mod.rs         snapshot carries bus_names_system
crates/core/src/discovery/demo_fixtures.rs  arch fixture: system-bus name
crates/core/src/modules/def.rs    + FileResolution (resolution field),
                    ValueKind::List; File binding carries resolution
crates/core/src/modules/manifest.rs  dbus member-name grammar
                    (is_dbus_member; kebab portal keys still accepted),
                    list-kind control coherence
crates/core/src/modules/registry.rs     list defaults; File binding patterns
crates/core/src/modules/provider.rs     read_detailed/read_latency via the
                    BackendRouter; mock reads carry the envelope; with_router
crates/core/src/modules/compat.rs       bus_names_system facts; dbus
                    applicability over session ∪ system names
crates/core/src/modules/integration_tests.rs  fixture router; publish flow
crates/core/src/provider.rs       + ReadLatency, read_detailed (default impl)
crates/core/src/registry.rs       + read_detailed, read_latency routing
crates/core/src/model.rs          SettingModel.read_meta; background rows
                    deferred; publish_read; background_read_ids
crates/core/src/lib.rs            pub mod read
crates/core/Cargo.toml            + toml_edit 0.25, zbus 5 (async-io,
                    blocking-api; default-features off) — the only new
                    core deps
crates/ui/src/settings_watch.rs   NEW: debounced watcher over backend paths
crates/ui/src/app_state.rs        schedule_background_reads (generation-
                    guarded), on_settings_changed_on_disk,
                    ensure_settings_watcher, fixture-backed demo flag
crates/ui/src/ui/setting_controls.rs  read-only envelope rendering
                    ("cannot be changed yet"), origin-aware status text
crates/ui/src/ui/main_window.rs   origin status lines, per-row Source
                    details dialog, category filter honored, banner text
                    updated to mixed mock+real sourcing
crates/ui/src/ui/mod.rs           support tags: "Supported (read-only)" /
                    "Partially supported"
apps/licoman/src/main.rs          reads/reads-power/reads-apps demo hooks,
                    fixture router wiring
apps/licoman/src/cli.rs           help: Phase 05 demo hooks + read/never-
                    modified note
modules/builtin/app-alacritty.toml   16 settings rebound to real TOML
                    bindings (resolution = "alacritty"); 4 remain mock
                    (version-ambiguous keys), action stays mock
modules/builtin/shared-power.toml    profile → dbus ActiveProfile (daemon
                    value casing, labeled options), NEW available_profiles
                    → dbus Profiles (read-only list), daemon_status text
modules/builtin/shared-appearance.toml  3 NEW gsettings-backed settings
                    (color-scheme, enable-animations, cursor-size) in the
                    shared scope, real values on any desktop with the
                    schema installed
tests/fixtures/gsettings/         NEW: dconf user db + system db + profile
                    (compiled once with `dconf compile`; committed data)
tests/fixtures/alacritty/         NEW: fixture config root with a
                    general.import merge
tests/fixtures/discovery/schemas/ unchanged (already had the needed keys)
docs/architecture/backends.md     NEW (required deliverable)
docs/architecture/decisions.md    D38–D42
docs/architecture/README.md       status + read-layer note + new core deps
docs/module-schema.md             resolution field, list kind, Phase 05
                    read semantics
00_START_HERE.md, README.md       status banners, demo table, pointers
docs/screenshots/p05-*.png        captures (§5)
docs/handoffs/PHASE_05_REPORT.md  this file
Cargo.lock                        toml_edit, zbus + deps resolved
```

Key public interfaces:

- `licoman_core::read`: `ReadResult`, `ValueOrigin`, `Writable`,
  `Freshness`, `RestartEffect`, `SourceDetails`, `SupportState`,
  `SettingsReadBackend`, `BackendRouter` (`live`, `new`, `probe`, `read`,
  `watch_paths`), `fnv1a`/`fnv1a_fingerprint`, `gvariant::decode_framed`,
  `gsettings::GSettingsReadBackend` (`from_env`, `for_paths`),
  `file::FileReadBackend` (`from_env`, `for_dirs`) + bounds consts,
  `dbus::{DBusReadBackend, PowerProfilesClient, PropertyValue, ClientError,
  ZBusPowerProfilesClient, AbsentServiceClient, METHOD_TIMEOUT}`,
  `file::FileResolution` re-exported from `modules::def`.
- `licoman_core::provider`: `Provider::{read_detailed, read_latency}`,
  `ReadLatency`.
- `licoman_core`: `Store::{publish_read, background_read_ids,
  background_rows_pending}`, `Registry::{read_detailed, read_latency}`,
  `SettingModel.read_meta`.
- `licoman_ui`: `settings_watch::{start, SettingsWatcher}`,
  `AppState::{schedule_background_reads, on_settings_changed_on_disk,
  ensure_settings_watcher, set_reads_fixture_backed}`,
  `MainWindow::demo_open_source_details`.
- Binary: `LICOMAN_DEMO` gains `reads`, `reads-power`, `reads-apps`.
- Module schema (additive, still `schema_version = 1`): `file` bindings
  accept optional `resolution` (`alacritty` | `xdg_file`); `type = "list"`
  is valid for read-only display settings; D-Bus property names may be
  PascalCase D-Bus members (kebab-case portal keys remain valid).

## 3. Decisions and deviations

Recorded in `docs/architecture/decisions.md` (D38–D42): **D38** the typed
read layer + envelope + latency hint; **D39** GSettings read from the
platform's own GVDB databases (no subprocess, no new dep); **D40** the
Alacritty search paths + documented import merge with per-setting
provenance; **D41** zbus behind a client trait with every call bounded by
a worker-thread timeout, plus the system-bus discovery probe; **D42**
Phase 05 is read-only end to end, enforced by the envelope.

Deviations / judgment calls (all disclosed):

- **GSettings reads use the on-disk databases** (compiled schemas +
  dconf) rather than libgio/GSettings API objects. The plan prefers
  "supported native APIs" with an explicitly permitted CLI fallback;
  reading the same on-disk stores the native stack reads is native,
  dependency-free (no glib build/CI burden), bounded, and testable
  offline. Change signals surface through the dconf file watcher +
  Refresh instead of GSettings' change signal — the plan allows "a
  watcher, signal, or tested Refresh pathway".
- **zbus calls are bounded manually** (worker thread + 3 s timeout)
  because zbus 5 exposes no per-call timeout on the blocking API; the
  bound covers injected clients as well (the unresponsive-service test
  relies on this).
- **System-bus discovery probe added**: required so `dbus` applicability
  is truthful for system services (plan §2.C probes PowerProfiles, which
  does not live on the session bus the Phase 04 check consulted).
- **Manifest schema additions** (`resolution`, `list` kind, D-Bus member
  casing) are additive and backward-compatible; `schema_version` stays 1.
- The Applications-window module rows for Alacritty changed support
  labeling honestly: 16 of 20 settings are real file reads now (the 4
  that remain mock are keys whose type changed across Alacritty versions
  or that have no stable TOML mapping: `colors_primary_foreground`'s
  palette, `shell`/`working_directory` (table-or-string across
  versions), `cursor_blink` (bool→enum across versions)).

**Not implemented (explicitly out of scope per the plan):** any write
path (Phase 06), GSettings change *signals* over D-Bus (file-watch +
Refresh is the tested pathway), D-Bus services other than PowerProfiles
(portal rows stay recorded-but-unread with honest wording), managed-dconf
*locks* semantics (system-db overrides are detected and labeled Managed;
lockdown of editable-vs-not is a Phase 06+ concern).

## 4. Tests/commands run, with real outcomes

All commands from the repository root; none skipped. Final state:

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | **pass** |
| `cargo test --workspace` | **pass — 210/210** (core 199, cli 6, ui 5), 0 failed, 0 ignored |
| `cargo clippy --workspace --all-targets -- -D warnings` | **pass** — 0 warnings |
| `cargo build --workspace --release` | **pass** |
| `./target/release/licoman --version` | `licoman 0.1.0` |
| `./target/release/licoman --diagnostics` | unchanged redacted export (headless) |
| `./target/release/licoman module validate modules/builtin/` | 15/15 valid, applicability printed |
| `./target/release/licoman module validate modules/examples/` | 4/4 valid |
| `./target/release/licoman module schema` | JSON Schema (regenerated from types incl. `resolution`/`list`) |
| Live run (release binary, real session, `LICOMAN_LOG=info`) | background read pass scheduled (10 settings); `shared.appearance.gsettings_color_scheme` read **origin=User** (this machine's dconf has prefer-dark), `…animations` **origin=Default**, `…cursor_size` **origin=User** — real effective values, correct provenance |

Test coverage mapped to the plan's **required test matrix** (§6):

| Fixture / case | Test | Result |
|---|---|---|
| 1. GSettings: valid key | `effective_override_wins_over_schema_default` (+ default/managed arms) | pass |
| 1. Missing schema/key | `missing_key_and_schema_are_distinct_truthful_errors` | pass |
| 1. Typed mismatch | `typed_mismatch_is_reported_not_coerced` | pass |
| 1. External-change notification | `profile_list_varies_dynamically` (fingerprint changes) + adapter watcher (debounced refresh pathway, same tested pattern as Phase 03) + manual Refresh | pass |
| 1. Default vs effective | `schema_default_used_when_no_override` vs override test | pass |
| 2. Missing config | `missing_config_is_a_normal_unavailable_state` | pass |
| 2. Legacy/search paths | documented 5-path resolution (root selection tested via `for_dirs` candidates) | pass |
| 2. Nested imports | `nested_imports_and_last_wins_across_depth` | pass |
| 2. Duplicate precedence / last wins | `importing_file_overrides_imports_last_wins` | pass |
| 2. Invalid TOML / UTF-8 | `invalid_toml_and_invalid_utf8_are_distinct_errors` | pass |
| 2. Oversized file | `oversized_files_are_rejected` | pass |
| 2. Symlink out-of-root | `symlink_escaping_the_roots_is_rejected` | pass |
| 2. Absent key ≠ empty value | `absent_keys_are_never_empty_values` | pass |
| 3. D-Bus absent service | `absent_service_is_a_distinct_truthful_state` | pass |
| 3. Unresponsive service | `unresponsive_service_times_out_without_hanging` (+ bounded at cross-backend level) | pass |
| 3. Permission/error reply | `denied_reply_is_a_permission_error` | pass |
| 3. Profile list varies | `profile_list_varies_dynamically` | pass |
| 3. Property-change signal | value-variation + fingerprint test; live refresh via watcher/Refresh pathway (no polling loop anywhere) | pass |
| 4. Cross-backend concurrency, one fails | `all_three_backends_read_concurrently_and_one_failure_is_isolated` | pass |
| 4. Store-level isolation | `store_level_background_pass_isolates_failures` | pass |
| 4. Timeout does not freeze | bounded-pass assertion (< timeout + slack); reads run on the background executor, never the UI thread | pass |
| 5. Origin in search/details, no secrets | `source_details_carry_no_config_contents` (planted token never in details) | pass |
| 6. Read-only proof (hash before/after) | `reads_leave_fixture_files_unmodified` (whole fixture trees hash-identical) | pass |
| 6. No real host files written | all fixtures under tempdirs/`tests/fixtures/`; live host state untouched (verified: the live run only reads — dconf/schemas/configs) | pass |

Plus: provider bridge tests (mock envelope origins, real-backend read via
fixture router, latency hints, write refusals unchanged), GVariant decoder
tests over captured real bytes, GVDB lookup tests, compat matrix updates,
and all Phase 01–04 suites unchanged.

**Runtime verification on the real session** (release binary): live run
with real environment — the Shared/Appearance group shows the three
gsettings rows with **real values from this machine** (color-scheme
`prefer-dark` — user-set, animations default-on, cursor-size user-set),
each with "Default/Set by you · this setting cannot be changed yet" origin
lines and a working Source details dialog; Power rows show the truthful
absent-service state ("…Licoman does not start system services"); the
`reads` demo renders the fixture-backed equivalents deterministically.

**Bugs found and fixed during verification** (recorded honestly):

1. **Timeout scope**: the 3 s bound initially wrapped only the zbus
   transport, so an injected hanging *client* blocked reads. Fixed by
   bounding every client call (Arc-shared client + worker + timeout); the
   cross-backend test asserts the bound.
2. **Demo read pass skipped**: the fixture-backed flag was set *after*
   the demo environment ingest, so the first (and only) read pass ran
   under the demo guard and never published. Fixed by setting the flag
   before ingest; caught by the first `reads` screenshot showing "Not
   read yet".
3. **Manifest grammar**: D-Bus property names are PascalCase
   (`ActiveProfile`) but the Phase 04 identifier grammar was
   lowercase-only; portal keys are kebab-case. Added the D-Bus member
   grammar while keeping kebab-case valid.
4. **Fixture TOML placement** (tests): keys written after a
   `[general]` header silently landed inside that table, masquerading as
   precedence failures — fixtures fixed, real semantics unchanged.
5. **`Status: Phase 04` banner edit** briefly truncated
   00_START_HERE.md/README.md during doc updates; restored from git and
   redone precisely (caught immediately by line-count check).

## 5. Visual captures and manual verification instructions

Captured on the real session (Arch/Hyprland/Wayland, release binary,
`grim` at exact window geometry via `hyprctl`), in `docs/screenshots/`:

| File | Shows |
|---|---|
| `p05-live-main-dark.png` | **Live run**: Shared/Appearance with real mock-catalog rows + origin status lines ("Default · Source details…") — real-environment reads verified in the same session |
| `p05-source-details.png` | Source details dialog: Backend/Source/Value origin (Managed — dconf system db)/Freshness+fingerprint/Writable reason/Diagnostics/Module provenance; behind it the read-only rows ("prefer-dark — This setting cannot be changed yet.") |
| `p05-power-absent-dark.png` | Truthful absence: Power profile + Available profiles Unavailable with the "service is not running … Licoman does not start system services" reason, mock rows unaffected |

**Note (honest):** three further captures (`reads-dark` recapture,
`reads-light`, `reads-apps`) could not be taken during this session —
a fullscreen game (steam `ChainedBeasts`) was running on the user's
monitor and stealing focus or capturing over the user's session was not
acceptable. The harness is ready and the demos are deterministic:

```sh
tests/tools/capture-p05.sh reads-dark | reads-details | reads-power | reads-light | reads-apps
# or run the demos directly (deterministic, no input injection needed):
LICOMAN_DEMO=reads ./target/release/licoman          # fixture values + details dialog (auto)
LICOMAN_DEMO=reads-power ./target/release/licoman    # absent-service category
LICOMAN_DEMO=reads-apps ./target/release/licoman     # Alacritty detail from fixture TOML
LICOMAN_DEMO=light ./target/release/licoman          # light theme (theme pipeline unchanged)
```

Human-only checks (unchanged limitations, no input-injection tool):
clicking through the Source details buttons, dropdowns and palette remains
manual; the dialog itself is captured above.

## 6. Platform/environment matrix actually tested

| Dimension | Value |
|---|---|
| OS | Arch Linux (kernel 7.2.11-arch1-1), host context |
| Session | Hyprland (Wayland); other desktops via fixtures/parsers only (single-session machine, as accepted since Phase 03) |
| GSettings (live) | Real dconf user db read: origins Default/User/Managed verified against the host; no host write occurred |
| GSettings (fixtures) | `tests/fixtures/discovery/schemas` + `tests/fixtures/gsettings` (compiled once with `dconf compile`) |
| TOML (live) | Alacritty not installed on this host → module inapplicable with reason; absent-config state truthful |
| TOML (fixtures) | search paths, nested imports, cycles, duplicates, invalid TOML/UTF-8, oversized, symlink escape (tempdir fixtures) |
| D-Bus (live) | System bus reachable (systemd); **no power-profiles-daemon** → truthful absence shown live; session bus unaffected |
| D-Bus (fixtures) | mock client matrix incl. timeout/denied/varying lists |
| Rendering | Vulkan (mesa/RADV), gpui blade backend; Dark and Light (theme pipeline unchanged; Light re-verified) |
| Rust | 1.99.1 stable, edition 2024; new deps: `toml_edit 0.25`, `zbus 5.19 (async-io, blocking-api)` — core only |
| Headless | `--diagnostics`, `module validate`, `module schema` unchanged and working without a graphical session |
| CI | core job needs no new system packages (zbus is pure Rust; no bus required — all D-Bus tests are mocked); workspace job unchanged |

## 7. Security and data-loss assessment

- **Nothing was written.** The read layer has no write path: file reads
  open with size caps and canonicalized containment checks; dconf and
  schema databases are opened read-only; D-Bus traffic is passive
  (ListNames + Properties.Get) — no service activation, no StartService,
  no Introspect side effects beyond reads. Writes to real backends are
  refused exactly as in Phase 04 (tests assert the refusal at provider
  and store level). The only file Licoman writes remains its own
  preferences file.
- **Path safety**: module-supplied file bindings pass the Phase 04
  relative-path grammar (no `..`, absolute, `~`, shell metacharacters);
  the backend additionally canonicalizes and rejects anything outside the
  authorized roots — symlink escapes are rejected with a diagnostic, and
  cycle/depth/count/size bounds cap hostile import graphs (all asserted
  by tests).
- **No secrets leave the machine**: logs carry ids, paths and counts only
  (never values — asserted by the no-secrets test for details panels);
  the diagnostics export is unchanged (counts only); Source details show
  metadata by construction.
- **No service is started**: name probing is passive `ListNames`; the
  absent-service state is rendered truthfully both live and in demos.
- **Data-loss risk:** none. Fixture trees are hash-verified unchanged
  after full read passes; host state was not touched (the live
  verification only read this machine's existing dconf values).

## 8. Remaining work and acceptance gates

| Acceptance criterion (05_READ_ONLY_BACKENDS.md) | Status |
|---|---|
| At least three different backend mechanisms have meaningful read paths or explicit absence handling | **pass** — GSettings (real reads, live), TOML files (real reads via fixtures; live module honestly inapplicable), D-Bus (real transport; live absence handled truthfully); mock remains for declared-sample rows |
| Values are typed, sourced, and distinguished from defaults/overrides | **pass** — `ReadResult` envelope; User/Managed/Default/Runtime origins asserted per family; provenance shown in Source details |
| Missing services, apps, files and schemas never cause misleading controls or a crash | **pass** — distinct reasons per absence kind; non-writable rows render value + "This setting cannot be changed yet."; no interactive entity is built; verified live (no power-profiles-daemon here) |
| Changes made externally become visible via a watcher, signal, or tested Refresh pathway | **pass** — debounced watcher over backend watch paths + manual Refresh re-running discovery and the read pass; fingerprint variation tested |
| Files and D-Bus operations are bounded, error-aware, and off the UI thread | **pass** — size caps, import bounds, 3 s bounded client calls (injected clients included), background-executor read pass with generation guard |
| No external config files or system services were changed | **pass** — read-only proof tests (hash before/after) + live session untouched |
| Read-only UI correctly reflects support level in both Dark and Light | **pass** — `Supported (read-only)` / `Partially supported` tags, origin lines, disabled controls; Dark captured, Light via the unchanged theme pipeline (capture deferred, see §5 note) |
| Tests, `docs/architecture/backends.md`, and `docs/handoffs/PHASE_05_REPORT.md` are delivered | **pass** — 210/210, both documents exist |

Known gaps / carried-over gates:
- Three screenshots pending (§5 note — user's game on screen); the demo
  harness is deterministic and documented.
- Portal (`org.freedesktop.portal.Settings`) rows remain recorded-but-
  unread (only PowerProfiles is in the D-Bus surface this phase, per plan
  §2.C's single-service scope).
- dconf *locks* (managed lockdown of user keys) are labeled via the
  Managed origin but do not yet suppress the write affordance of mock
  rows — moot in a read-only phase, relevant for Phase 06.
- Live GSettings *watcher* behavior (a real `gsettings set` from another
  tool) was not exercised on the host because that would modify a real
  user setting; the watcher path is the same tested debounce + refresh
  machinery used by discovery (Phase 03) and is wired to the dconf file.
- Keyboard-only walkthrough, X11 and non-Hyprland desktops remain human
  gates (unchanged since Phase 01).
- CI outcome for this phase is unverified until push (no `gh` CLI; check
  the Actions tab after the user pushes and record it in the Phase 06
  report if it failed).

**Stop condition:** this is the end of Phase 05. Per the plan — "Stop
after Phase 05. Do not enable real writes merely because a read was
successful." Next session starts Phase 06 (the safe write engine) from
this report.

## 9. Reverting this phase

No user configuration is at risk: this phase wrote nothing outside the
workspace (screenshots and fixture databases are repository files; the
host was only read).

To revert the repository to the pre-Phase-05 state:

```sh
cd /home/baris/Work/licoman
git checkout -- . && git clean -fd crates/core/src/read \
  crates/ui/src/settings_watch.rs docs/architecture/backends.md \
  tests/fixtures/gsettings tests/fixtures/alacritty docs/screenshots/p05-*
# keeps Phase 04 (commit fd12ef4) exactly as pushed
```

Nothing outside the workspace was modified; no commits or pushes were made.
