# Phase 04 Report — Declarative Module Schema and Settings Registry

**Date:** 2026-10-31
**Workspace:** `/home/baris/Work/licoman` (`main`, Phase 03 commit + this
phase's uncommitted changes — nothing was committed or pushed by the
automated session, per the execution contract; the tree is ready for review).
**Executor:** automated session (ZCode), one phase per session as required.
**Plan source:** `04_MODULE_SCHEMA_AND_REGISTRY.md` (author's document pack).
All nine section headings of the required handoff format are below.

---

## 1. Goal and actual scope implemented

**Goal (from the plan):** make new app/desktop/distro integrations possible
**without editing the UI**, through a stable, validated module format. Hard
boundary honored: modules can describe settings but cannot run arbitrary
code, write host files, or elevate privileges; no real setting was read or
written (mock reads only).

### The module system (`crates/core/src/modules/`, new, 8 files, ~3.9k lines)

- **Canonical types** (`def.rs`): `ModuleDefinition`, `SettingDefinition`,
  `BackendBinding`, `Compatibility`, `SupportInfo`, `AvailabilityPredicate`,
  `ManifestValue` — serde TOML + `schemars` JSON-Schema derives. Unknown
  fields are rejected everywhere (typos fail loudly).
- **Loader/validator** (`manifest.rs`): TOML parse with byte-span →
  line/column conversion; semantic validation producing **all** issues in one
  pass with field-level locations; id grammars; shell-metacharacter and
  path-escape rejection; bounds (256 KiB/file, 512 settings/module, bounded
  strings/keywords/options).
- **Applicability** (`compat.rs`): pure evaluation over `EnvironmentFacts`
  (extracted from the Phase 03 snapshot) with **distinct, preserved reasons**
  per dimension: desktop (incl. conservative unknown-desktop), distro,
  application (not installed / version unknown), backend capability, and
  per-setting predicates; per-setting binding checks against the **actual
  schema→key list and session-bus names** — a missing GSettings schema/key
  or absent service makes the setting unavailable, never a fake value.
- **Registry** (`registry.rs`): load → validate → index (module ids, full
  setting ids, descriptors, provenance lines, application-scope summaries)
  with deterministic ordering; duplicate module ids reject the later
  definition (never last-one-wins) while unrelated modules load; rejected
  modules are recorded with their diagnostics. Built-in manifests are
  **embedded at compile time** (`builtin.rs`) — trusted at build time, no
  runtime override channel (D31).
- **Provider bridge** (`provider.rs`): `ModuleProvider` implements the
  Phase 02 `Provider` trait so the existing `Store`, command vocabulary and
  rendering paths work unchanged (D33). Mock values are served **only from
  declared defaults** (or explicit not-set for nullable string/path);
  non-mock backends fail closed on read ("Not readable yet — Phase 05") and
  **unconditionally on write** (D36). One deterministic id
  (`shared.network.dns`) fails mock writes to keep the failure flow live.
- **Integration tests** (`integration_tests.rs`): 1,200-setting volume
  catalog, app-removed-mid-session, store edit/apply over manifest settings,
  navigation/edits surviving registry refresh, immediate-policy applies,
  and the real-backend write boundary.

### Environment facts retained (Phase 03 extension)

`EnvironmentSnapshot` now keeps `gsettings_schemas` (schema → declared keys,
parsed by the existing GVDB reader) and `bus_names` (session-bus well-known
names) — previously counted and discarded; they are what module applicability
checks bind against. The synthetic schema fixture gained
`org.gnome.desktop.interface` (12 keys, XML source updated, GVDB recompiled
with `glib-compile-schemas`) so the Fedora/GNOME demo grounds the GNOME
module. The diagnostics export is unchanged (counts only).

### Descriptor/store evolution

- `SettingDescriptor` fields are owned (`String`) and carry `keywords` +
  `module_id` provenance (D32); `Scope::Application` exists but is offered
  only in the Applications window; `Control::Action` is inert by design.
- `search.rs`: precomputed normalized-text `SearchIndex` (keywords, module
  ids, breadcrumbs) — no per-keystroke list reconstruction.
- `Store`: precomputed index, `reload_all()` which restores staged edits for
  surviving ids and drops them for now-unavailable ones; selections (scope/
  category/app/tabs) live in separate fields and survive reloads untouched.

### UI: data-driven, one control factory

- **`ui/setting_controls.rs` (new)**: `ControlSet` builds the interactive
  entities (Select/Slider/Number/Text/Path inputs) once per window from
  descriptors, subscribes them to typed `Command::Edit` dispatches, re-syncs
  from the store, **rebuilds when module applicability changes**, and renders
  Switch widgets (stateless) plus truthful read-only/action presentations —
  exclusively through the existing Quiet Control components. No window
  hard-codes a setting id any more.
- **MainWindow** and **AppsWindow** are fully registry-driven: rows come from
  descriptors, Applications detail tabs from the module's settings `group`
  metadata, "Source details…" shows recorded provenance (bindings, docs,
  tested versions, maintainer). Search-palette results show an apply-status
  tag; application-scope hits navigate into the Applications window by
  stable identity. Removed-mid-session apps keep the graceful
  "no longer in the list" fallback.
- The Applications window merge (`matching.rs`) now consumes registry app
  modules (`AppModuleRef`) instead of the deleted hand-written catalog;
  support levels gain an honest `read_only` tag.

### Manifest data (201 shipped settings)

`modules/builtin/` — 15 modules across all four scopes (shared×6, desktop×3,
distribution×2, application×4) exercising every control kind
(switch/checkbox, dropdown, number, slider, text, path, action, read-only),
restart/auth/risk metadata, gsettings/dbus/file/mock bindings. Legacy
`MockProvider::standard()`, `APPS_CATALOG` and `MOCK_ENVIRONMENT` are gone
(the hand-written provider remains only as the store-lifecycle test fixture).
`modules/examples/` — mock application, GSettings example, read-only service
example, and a module that intentionally fails compatibility (desktop +
version-unknown reasons, both preserved).

### CLI

- `licoman module validate <path>...` — read-only linter: parses/validates
  files or directories (`*.toml`), prints located diagnostics and, computed
  against a live read-only discovery snapshot, each module's applicability
  with reasons; exit 1 on invalid manifests; never modifies anything.
- `licoman module schema` — JSON Schema (draft 2020-12) generated from the
  canonical types (D35). `--help` documents both.

**Not implemented (explicitly out of scope per the plan):** real backend
reads (Phase 05), any write path (Phase 06), external/runtime module loading
(deferred by the plan until a separate security review — D31), reset-to-
default actions (defaults are recorded; a real reset needs backend
semantics), rendering enum option *labels* in dropdowns (labels are parsed,
validated and indexed; dropdowns render values in this phase).

## 2. Changed/added files and public interfaces

```text
crates/core/src/modules/          NEW (8 files, ~3.9k lines):
  mod.rs            docs, re-exports, builtin-catalog validity/volume tests
  def.rs            canonical types (+ serde, schemars)
  manifest.rs       TOML loader + validator (located issues) + tests
  compat.rs         EnvironmentFacts, applicability, per-setting checks
  registry.rs       ModuleRegistry: load/index/provenance/app modules
  provider.rs       ModuleProvider (Provider impl) + fail-closed tests
  builtin.rs        embedded manifest table (15 files)
  integration_tests.rs  volume/app-removal/store-flow/boundary tests
modules/builtin/*.toml            NEW: 15 manifests, 201 settings
modules/examples/*.toml           NEW: 4 annotated examples
crates/core/src/descriptor.rs     String fields, Scope::Application,
                                  Control::Action, keywords/module_id
crates/core/src/search.rs         precomputed SearchIndex
crates/core/src/model.rs          search index, reload_all, rows_for_ids,
                                  staged-edit log line
crates/core/src/{value,registry,mock}.rs  owned fields; mock.rs keeps only
                                  the lifecycle-test provider
crates/core/src/discovery/{mod,backends}.rs  BackendFacts retained in the
                                  snapshot (schemas+keys, bus names)
crates/core/src/discovery/matching.rs        merge over AppModuleRef
crates/core/src/discovery/demo_fixtures.rs   schema/bus facts for demos
crates/core/src/discovery/fixtures_tests.rs  updated fixture counts
crates/core/src/lib.rs            `pub mod modules`
crates/core/Cargo.toml            + schemars = "1" (only new core dep)
crates/ui/src/ui/setting_controls.rs  NEW: ControlSet factory
crates/ui/src/ui/main_window.rs   registry-driven rows, palette nav/status
crates/ui/src/ui/apps_window.rs   registry-driven detail, tabs, provenance
crates/ui/src/ui/mod.rs           SupportLevel tag, generic group titles
crates/ui/src/app_state.rs        ModuleProvider wiring, apply_environment,
                                  AppEvent::ModulesChanged
apps/licoman/src/{cli,main}.rs    module validate/schema subcommands,
                                  module-registry bootstrap, demo hooks
                                  (dirty/app-detail re-timed)
apps/licoman/Cargo.toml           + schemars, serde_json (schema printing)
tests/fixtures/discovery/schemas/ org.gnome.desktop.interface added to the
                                  synthetic fixture (XML + recompiled GVDB)
docs/module-schema.md             NEW: authoring guide + field reference
docs/architecture/decisions.md    D31–D37
docs/architecture/README.md       status + module-registry note
00_START_HERE.md, README.md       status, demo table, module tooling
docs/screenshots/p04-*.png        8 captures (§5)
docs/handoffs/PHASE_04_REPORT.md  this file
Cargo.lock                        schemars, serde_json resolved
```

Key public interfaces:

- `licoman_core::modules`: `ModuleRegistry` (`load_builtin`, `load`,
  `set_environment`, `setting_status`, `setting`, `module`,
  `setting_ids_of_module`, `descriptors`, `app_modules`, `provenance_of`,
  `rejected`), `ModuleProvider` (`new`, `set_environment`, `with_registry`,
  `Clone`), `ModuleDefinition`/`SettingDefinition`/`BackendBinding`/…,
  `ModuleApplicability`/`IncompatibilityReason`/`EnvironmentFacts`,
  `parse_manifest`/`validate_definition`/`ManifestIssue`, `AppModuleRef`,
  `ModuleSource`, `descriptor_for`.
- `licoman_core`: `Scope::Application`, `Control::Action`, descriptor
  `keywords`/`module_id`, `search::SearchIndex`, `Store::{reload_all,
  rows_for_ids}`, `EnvironmentSnapshot::{gsettings_schemas, bus_names}`,
  `discovery::backends::BackendFacts`.
- `licoman_ui`: `ui::setting_controls::{ControlSet, phase_status_text}`,
  `AppEvent::ModulesChanged`, `AppState::{module_provider,
  select_identity_for_module}`.
- Binary: `module validate <path>...`, `module schema`; `LICOMAN_DEMO` gains
  `app-detail`; `--help` documents all of it.
- Changed: `Command::SelectApp` semantics unchanged (stable id) but module
  identities are now namespaced (`app.mpv`); `Support` moved from
  `mock` to `modules::def::SupportLevel` (adds `ReadOnly`).

## 3. Decisions and deviations

Recorded in `docs/architecture/decisions.md` (D31–D37): **D31** built-ins
embedded at build time, no runtime search roots; **D32** descriptors are
runtime data (+ `Scope::Application`, `Control::Action`); **D33** the
registry reaches the store through a `Provider`, conditional reload
preserves navigation; **D34** applicability reasons are distinct preserved
data (version-unknown is conservative); **D35** JSON Schema generated via
`schemars` (the one new core dependency); **D36** mock value-binding rules
(default required, nullable restricted, deterministic fail-write fixture);
**D37** settings-level `group` drives Applications tabs (module-level
`group`/`order` also parsed, as the plan's table lists them there).

Deviations from the plan's *illustrative* TOML (which the plan explicitly
marks as "the proposed shape, not a drop-in parser interface"):

- The apply-policy field is named `policy` (matching the plan's §1 field
  table) rather than the example's `apply_policy`.
- Enum options are declared as `[[settings.options]]` value/label tables
  (labels power validation/search now, dropdown rendering in a later phase).
- `docs_url` "verified" is interpreted as format-validated (`https://`,
  whitespace-free) at load; content verification remains a review concern.

No other deviations. The plan's contingency "avoid overengineering a public
runtime-ABI or dynamically loaded plugin system" was followed: complex
integrations stay compiled Rust behind `Provider`.

## 4. Tests/commands run, with real outcomes

All commands from the repository root; none skipped. Final state:

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | **pass** |
| `cargo test --workspace` | **pass — 175/175** (core 164, cli 6, ui 5), 0 failed, 0 ignored |
| `cargo clippy --workspace --all-targets -- -D warnings` | **pass** — 0 warnings |
| `cargo build --workspace --release` | **pass**, 0 warnings |
| `./target/release/licoman --version` | `licoman 0.1.0` |
| `./target/release/licoman --help` | documents module commands |
| `./target/release/licoman --diagnostics` | unchanged redacted export (arch/hyprland) |
| `./target/release/licoman module validate modules/examples/` | 4/4 valid; applicability printed with reasons (see §5) |
| `./target/release/licoman module validate modules/builtin/` | 15/15 valid; per-module applicability printed |
| `./target/release/licoman module schema` | JSON Schema draft 2020-12 (validated as JSON) |

Test coverage mapped to the plan's **required test matrix** (§7):

| Fixture / case | Test | Result |
|---|---|---|
| Valid built-in module | `builtin_manifests_are_valid` + `builtin_catalog_has_200_settings…` | pass |
| Duplicate module ID | `duplicate_module_id_rejects_the_later_definition_and_keeps_others` | pass |
| Duplicate setting IDs within a module | `duplicate_setting_ids_are_rejected_not_last_one_wins` | pass |
| Unsupported schema version | `unsupported_schema_version_is_an_explicit_rejection` | pass |
| Unknown control/backend type | `unknown_control_and_backend_kinds_are_rejected` | pass |
| Invalid range/default/value type | `invalid_range_and_default_types_are_field_level_errors` | pass |
| Missing GSettings schema | `missing_gsettings_schema_or_key_makes_the_setting_unavailable` (+ key-missing arm) | pass |
| Unknown app version | `app_not_installed_and_version_unknown_are_distinct_reasons` | pass |
| 1,000+ mock settings | `high_volume_catalog_search_and_navigation` (1,200 settings: unique ids, scope/category grouping, index search, identity navigation; finished < 0.1 s, asserted < 10 s) + `index_is_built_once_and_scanned_per_query` | pass |
| Unavailable app removed during session | `app_removed_mid_session_is_graceful` (staged edit dropped truthfully, reason preserved, no panic) | pass |
| Malicious path/shell payload | `malicious_path_and_shell_payloads_are_rejected` (`../`, absolute, `;`, backtick, `$()`, `~`) | pass |

Plus: provider fail-closed suite (mock reads serve defaults then written
values; `read_only`/`required_auth` refusals with distinct error kinds;
real-backend read/write refusals; pending-environment state), store-flow
integration (`volume_catalog_edits_flow_through_the_store`,
`immediate_policy_applies_on_edit`,
`real_backend_settings_never_activate_writes`,
`reload_preserves_navigation_and_edits_for_surviving_ids`), registry
ordering/indexing/provenance, compat dimension matrix (desktop/distro/
pending/facts extraction), validator grammar cases (unknown fields with
line/col, mock-default requirement, oversized file), search keyword/
provenance matching, matching determinism over `AppModuleRef`s, and the
Phase 02/03 suites unchanged (lifecycle, discovery, parsers).

**Runtime verification on the real session** (release binary): startup with
live discovery; interactive controls verified visually (§5); `dirty` demo
stages an edit through the manifest setting (`core::store: staged edit
id=desktop.session.gaps` in logs) and the apply bar appears; `failed` demo
still renders the failure flow; `--diagnostics` unchanged; no regression in
the headless-launch diagnostic.

**Bugs found and fixed during verification** (recorded honestly):

1. `SliderState`'s builder validates eagerly — building a slider whose
   `min` (1700) exceeded the *default* `max` (100) panicked mid-construction
   (caught by the fedora-gnome demo). Fixed by setting max before min, and a
   code comment explains the ordering constraint.
2. The control set was built while discovery was still pending (all rows
   unavailable) and never gained entities for rows that became editable.
   Fixed with an explicit rebuild on `AppEvent::ModulesChanged`.
3. The `dirty`/`app-detail` demo hooks raced discovery and a selection-only
   dispatch emitted no re-render; the hooks now wait for discovery and the
   app-detail hook notifies the window explicitly (interactive clicks always
   did this in their listeners).

## 5. Visual captures and manual verification instructions

All captures on the real session (Arch/Hyprland/Wayland, release binary,
`grim` at the window's exact geometry via `hyprctl`), in `docs/screenshots/`:

| File | Shows |
|---|---|
| `p04-main-window-dark.png` | Shared scope: registry-driven rows — dropdowns, switch, slider with unit — from built-in manifests, real footer |
| `p04-main-window-light.png` | Same catalog in Light (theme untouched by the registry) |
| `p04-desktop-scope.png` | Desktop scope: Session module with restart-requirement warning line, number stepper, external-conflict-labeled accent row |
| `p04-apps-window-dark.png` | Applications window: merged list with Supported/Partial tags, module-only rows honestly labeled |
| `p04-apps-detail-mpv.png` | mpv detail: registry-driven tabs (Playback/Video), sliders/switches, install-origin meta, **Source details…** |
| `p04-dirty-apply-bar.png` | "1 change pending" apply bar staged through a manifest setting |
| `p04-palette.png` | Search palette open; results carry apply-status tags and scope→category→setting breadcrumbs |
| `p04-demo-fedora-gnome.png` | Fedora/GNOME fixture: GNOME module applicable (footer "Fedora Linux 43 … · GNOME · Wayland") |

Reproduce:

```sh
cargo run -p licoman                              # live environment, dark
LICOMAN_DEMO=app-detail cargo run -p licoman      # registry-driven detail
LICOMAN_DEMO=dirty cargo run -p licoman           # staged edit + apply bar
LICOMAN_DEMO=fedora-gnome cargo run -p licoman    # GNOME module applicable
LICOMAN_DEMO=light cargo run -p licoman           # light theme
cargo run -p licoman -- module validate modules/examples/
cargo run -p licoman -- module schema
LICOMAN_LOG=info cargo run -p licoman             # registry reload logs
```

`licoman module validate` on this machine (abridged) demonstrates the
distinct-reason diagnostics exactly as the plan asks:

```text
modules/examples/mock-application.toml: ok — 3 settings; NOT applicable here
  (Requires "example-terminal.desktop" — none of them is installed on this system.)
modules/examples/gsettings-example.toml: ok — 2 settings; NOT applicable here
  (Requires "GNOME" — this system runs Hyprland.)
modules/examples/fails-compatibility.toml: ok — 1 settings; NOT applicable here
  (Requires "KDE" — this system runs Hyprland. Declares an application version
   requirement — Licoman cannot detect application versions yet, so the module
   is conservatively treated as incompatible instead of guessing.)
modules/examples/read-only-service.toml: ok — 1 settings; applicable to this environment
```

Human-only checks (unchanged limitations, no input-injection tool): clicking
through dropdowns/sliders/switches and the palette keyboard walkthrough
remain manual; the edit→apply flows themselves are covered by store-level
tests and the `dirty` demo capture.

## 6. Platform/environment matrix actually tested

| Dimension | Value |
|---|---|
| OS | Arch Linux (kernel 7.2.11-arch1-1), host context |
| Session | Hyprland (Wayland); X11/other DEs via fixture demos + parser tests only (single-session machine, as accepted since Phase 03) |
| Discovery (live) | 61 apps, D-Bus session bus, gsettings schema db, config probes — now also feeding module applicability |
| Module catalogs verified | 15 built-in (201 settings) + 4 examples via the live linter; GNOME module applicable on the fedora-gnome fixture, inapplicable-with-reason on this host |
| Rendering | Vulkan (mesa/RADV), gpui blade backend; Dark and Light verified |
| Rust | 1.99.1 stable, edition 2024; new deps: `schemars 1.2.3` (core), `serde_json` (binary, schema printing only) |
| Headless | `--diagnostics`, `module validate`, `module schema` all run without a graphical session |

## 7. Security and data-loss assessment

- **No manifest can execute anything.** The backend binding is a closed
  serde enum; there is no eval, no command field, no dynamic library
  reference anywhere in the module path. Every backend string passes an
  identifier/relative-path grammar that rejects shell metacharacters, path
  escapes (`..`, absolute, `~`, URI prefixes) and control characters —
  asserted by tests, so even a *future* consumer cannot receive such a
  value from a validated manifest. No process is spawned anywhere under
  `crates/core/src/modules/` (audited by grep during this phase).
- **No real settings are read or written.** The only value source is the
  declared-default mock; gsettings/dbus/file bindings are validated and
  recorded but reads return a truthful "not readable yet" error and writes
  return `WriteFailed` with "Nothing was modified" — asserted by
  `real_backend_reads_and_writes_fail_closed` and
  `real_backend_settings_never_activate_writes` at both provider and store
  level. The only file Licoman writes remains
  `$XDG_CONFIG_HOME/licoman/preferences.toml`.
- **File I/O added this phase:** the linter reads the paths the *user*
  passes (read-only, bounded by the 256 KiB limit per manifest); built-ins
  are embedded (no runtime file reads for the app's own catalog). The
  discovery watcher behavior is unchanged.
- **Privacy:** the snapshot now retains schema ids/keys and bus names in
  memory (service metadata, not file contents or user values). The
  diagnostics export still prints counts only; module provenance shown in
  the UI comes from manifests, not from the host. No telemetry.
- **Data-loss:** none. Staged edits live only in memory; a registry refresh
  preserves them unless their setting genuinely became unavailable (then
  the edit is dropped *because the setting can no longer be served* — the
  truthful alternative to a silent stale apply).

## 8. Remaining work and acceptance gates

| Acceptance criterion (04_MODULE_SCHEMA_AND_REGISTRY.md) | Status |
|---|---|
| A contributor can add a **mock** application module by adding a validated declarative file only, without changing GPUI source | **pass** — add a TOML to `modules/builtin/`, list it in `builtin.rs`, rebuild; every row/control/tab derives from the registry (the shipped app modules themselves prove the path; `modules/examples/mock-application.toml` is the template) |
| Every module, setting, category and search result has stable identity | **pass** — module ids, full namespaced setting ids (global uniqueness asserted), category strings within scopes, `SearchHit.id` + `module_id` |
| Applicability is based on actual detected environment/provider facts | **pass** — evaluation over snapshot facts (desktop identity, distro ids, installed app ids, capability ids, schema→keys, bus names); demo fixtures ground other environments |
| Unsupported settings are represented truthfully, not silently assigned defaults | **pass** — unavailable rows carry their preserved reason; mock values come only from declared defaults; pending environment is its own state |
| Invalid, duplicate, malicious or incompatible modules fail safely with diagnostics | **pass** — located issues, per-module isolation, duplicate rejection, payload grammar, preserved incompatibility reasons |
| No manifest may execute shell text or load native code | **pass** — closed binding enum + shell-safe grammar + no execution path; grep audit clean |
| Settings in both windows use Licoman's design tokens and current selected app theme | **pass** — both windows render through the same Quiet Control factory; light capture shows identical structure |
| Tests cover a high-volume synthetic module catalog and all negative cases | **pass** — 1,200-setting volume test + full §7 matrix (see §4) |
| `docs/module-schema.md`, examples and `docs/handoffs/PHASE_04_REPORT.md` exist | **pass** |

Known gaps / carried-over gates (pre-existing or explicitly deferred):
- External data-only module loading is **deferred by the plan** (security
  review) — the registry API is ready, the app does not wire it.
- Enum option *labels* are parsed/validated/indexed but dropdowns render
  values; label rendering is a small UI follow-up.
- Reset-to-default is recorded as metadata, not an action yet.
- Keyboard-only walkthrough remains a human gate (unchanged since Phase 01);
  X11 and non-Hyprland desktops verified via fixtures/parsers, not native
  sessions.
- CI outcome for this phase is unverified until push (no `gh` CLI; check
  the Actions tab after the user pushes and record it in the Phase 05
  report if it failed).

**Stop condition:** this is the end of Phase 04. Per the plan — "Stop after
Phase 04. Do not make a module-writable provider until it passes the
read-only and safety phases." Next session starts Phase 05 (read-only
backends) from this report.

## 9. Reverting this phase

No user configuration is at risk: this phase wrote nothing outside the
workspace (the schema fixture recompile happened inside
`tests/fixtures/discovery/schemas/`; screenshots are repository files).

To revert the repository to the pre-Phase-04 state:

```sh
cd /home/baris/Work/licoman
git checkout -- . && git clean -fd crates/core/src/modules crates/ui/src/ui/setting_controls.rs docs/module-schema.md modules docs/screenshots/p04-*
# keeps Phase 03 (commit acf7d07) exactly as pushed
```

Nothing outside the workspace was modified; no commits or pushes were made.
