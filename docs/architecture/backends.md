# Read-only configuration backends

How Licoman presents **actual effective settings** from fundamentally
different Linux configuration mechanisms without modifying anything
(05_READ_ONLY_BACKENDS.md). This phase is read-only; the only file Licoman
writes remains its own `$XDG_CONFIG_HOME/licoman/preferences.toml`.

## The typed read layer (`licoman_core::read`)

Every read produces one envelope, regardless of backend family:

```text
ReadResult {
    value: SettingValue,          // typed: bool/int/float/string/enum/path/list
    origin: ValueOrigin,          // Default | System | User | Managed | Runtime | Unknown
    writable: Writable,           // Yes | No{reason} | NeedsPermission{auth} | Unknown
    freshness: Freshness,         // read time + fnv1a64 fingerprint over source bytes
    restart_effect: RestartEffect,// None | App | Service | Session | System | Unknown
    details: SourceDetails,       // backend kind, exact target, derivation note,
                                  // imported flag, diagnostics — metadata only
}
```

Backends implement one trait:

```rust
trait SettingsReadBackend {
    fn kind(&self) -> &'static str;
    fn probe(&self, binding: &BackendBinding) -> SupportState;   // Available | Degraded | Unavailable
    fn read(&self, binding, descriptor) -> Result<ReadResult, LicomanError>;
    fn watch_paths(&self) -> Vec<PathBuf>;                       // for the debounced watcher
}
```

A `BackendRouter` maps a validated manifest binding (`mock` / `gsettings`
/ `dbus` / `file`) to its backend family. The `Provider` trait exposes
`read_detailed` plus a `read_latency` hint: mock reads are in-memory
(`Immediate`), real backends are `Background` — the store keeps those rows
in "Not read yet" and the UI adapter runs a **generation-guarded background
read pass** on the GPUI background executor, publishing row by row. A
hanging backend cannot stall the pass or the UI: every D-Bus client call
runs on a worker thread bounded by a 3 s timeout, and file reads are size-
capped.

Hard rules encoded here (not conventions):

- A missing setting is an error with a user-safe reason — never a valid
  empty value.
- Schema defaults, user overrides, managed overrides and runtime service
  state are distinguished by `origin`; the UI never claims "Set by you"
  for a default.
- `SourceDetails` carries identifiers and diagnostics only — never file
  contents or values — so the Source details panel cannot leak secrets.
- Nothing in this layer writes anything (see D42).

## GSettings / GLib backend (`read::gsettings`)

Reads the platform's own databases; no subprocess, no shell, no external
library.

- **Schema defaults** come from `gschemas.compiled` (GVDB), searched in
  XDG data-dir order (`GSETTINGS_SCHEMA_DIR`, `XDG_DATA_HOME`, then
  `XDG_DATA_DIRS` defaults). The parser resolves schema presence, the
  schema's `.path` attribute, and the per-key default value — stored as
  GLib's framed *non-normal* GVariant (`[body][pad][signature]`), decoded
  by a bounded decoder supporting scalars, strings, tuples (first-field
  decode for enum/range entries), variant and maybe; arrays/dictionaries
  are an explicit unsupported-type error.
- **Effective values** layer dconf overrides on top, in dconf's
  documented priority: the user database (`$XDG_CONFIG_HOME/dconf/user`)
  first, then the system databases listed in the dconf profile
  (`/etc/dconf/db/<name>` or `XDG_CONFIG_DIRS`). Overrides in a system db
  read as `Managed`, in the user db as `User`, otherwise the schema
  default is shown as `Default`.
- Relocatable schemas (no `.path`) read defaults-only with a degraded
  note. A non-dconf `GSETTINGS_BACKEND` adds a diagnostic. Changes are
  watched via the dconf user-db file (dconf-service rewrites it on every
  `gsettings set`), debounced by the adapter.

## Structured configuration files, TOML first (`read::file`)

The reference application is Alacritty, whose official configuration is
TOML:

- **Search paths** (Linux, in order): `$XDG_CONFIG_HOME/alacritty/alacritty.toml`,
  `$XDG_CONFIG_HOME/alacritty.toml`, `$HOME/.config/alacritty/alacritty.toml`,
  `$HOME/.alacritty.toml`, `/etc/alacritty/alacritty.toml` — first existing
  wins as the root file.
- **Imports** (`general.import`), per the official documentation:
  "Imports are loaded in order, skipping all missing files, with the
  importing file being loaded last" and "If a field is already present in
  a previous import, it will be replaced." The merge order is therefore
  `[imports in order (recursively), root file last]` and the **last
  loader wins**; provenance records the winning file and whether it was
  an import. A parsed first file is never presented as the effective
  config.
- **Safety bounds**: 256 KiB per file, import depth ≤ 8, ≤ 32 files per
  read, cycle detection along the current import chain (duplicate imports
  are legitimate; a file importing its own ancestor is skipped with a
  diagnostic), distinct errors for invalid TOML and invalid UTF-8, and a
  containment check on canonicalized paths — reads must stay inside the
  authorized roots (config home, home, `/etc/alacritty`), so a symlink
  pointing out of the roots is rejected, never followed.
- **Missing config is normal**: without a config file the row reports
  that the application runs on built-in defaults and shows no invented
  value (Licoman ships no validated per-version defaults). Keys absent
  from a present config are likewise "not present", not empty.
- `toml_edit` parses so the later write phase can preserve formatting.
  Non-TOML file bindings (e.g. Hyprland's keyfile) remain recorded but
  unread in this phase, with that exact wording.

## Service API / D-Bus backend (`read::dbus`)

- Client: `zbus` 5 (blocking API on worker threads; the `async-io`
  feature matches the ashpd feature set already in the dependency tree).
- Target: `org.freedesktop.UPower.PowerProfiles` — `ActiveProfile`
  (string) and `Profiles` (string array) properties, plus the legacy
  `net.hadess.PowerProfiles` name/path. Name probing is a passive
  `ListNames`; a service is never activated, installed, or started to
  make a demo look populated.
- Distinct states: service absent (not on the bus), bus unreachable,
  unresponsive (timeout), denied (permission error), and unexpected value
  types — each with its own user-safe message.
- Values read as `Runtime` origin. Property-change visibility follows the
  same refresh path (watcher / Refresh button re-reads; the bounded call
  design forbids tight polling loops).

## Cross-backend model

- One store, one lifecycle: every row — mock or real — moves through the
  same phases; real rows simply start "Not read yet" and fill in from the
  background pass. One backend failing leaves the others Idle (tested at
  router and store level).
- **Watchers**: the adapter watches `watch_paths()` (dconf user db,
  Alacritty config dir) with a 500 ms debounce; the manual Refresh button
  re-runs discovery and the read pass. Both update every open Licoman
  window through the same settings-changed event.
- **Source details**: the per-row disclosure shows backend type, exact
  schema/key or path, origin (with derivation note), freshness +
  fingerprint, writability, diagnostics, and the module's recorded
  provenance. Values stay in the row; secrets stay out of the panel by
  construction.

## Test coverage (05 §6)

| Case | Where |
|---|---|
| GSettings: valid key, missing schema/key, typed mismatch, default vs effective, system-db managed origin | `read::gsettings` tests (fixture schemas + dconf dbs) |
| External-change refresh | watcher debounce (adapter) + bounded refresh pass; fingerprint changes on override edits (dbus mock variation test) |
| TOML: missing config, search paths, nested imports, last-wins precedence, duplicate/cycle handling, invalid TOML, invalid UTF-8, oversized file, symlink escape, absent key | `read::file` tests (runtime tempdir fixtures) |
| D-Bus: absent service, unresponsive (timeout bounded), denied, varying profile list, non-PowerProfiles service | `read::dbus` tests (mock client) |
| Cross-backend: three families concurrently, one failing without affecting the others; timeout bounds the pass | `read::read_matrix_tests` |
| Read-only proof: fixture trees hash-identical before/after a full read pass | `read::read_matrix_tests` |
| No secrets: planted config token never appears in source details | `read::read_matrix_tests` |
| Store integration: background rows start Unknown, publish per row, failures isolated | `modules::integration_tests` |

Fixtures live under `tests/fixtures/` (`discovery/schemas`, `gsettings/`,
`alacritty/`); the dconf fixture databases were compiled once at authoring
time with `dconf compile` and are committed read-only test data. Running
the app or the test suite never touches real dotfiles or services.

## Phase 07 — write-capable backends (summary)

The read backends above gained write counterparts behind the Phase 06
engine; the full contract lives in `write-engine.md`. Summary:

| Family | Read (Phase 05) | Write (Phase 07) | Verification |
|---|---|---|---|
| `file` (Alacritty TOML) | self-contained GVDB/`toml_edit` parser, import merge | same approved-root resolution, `toml_edit` minimal patch, backups, atomic staging, xattr/ACL preserve-or-refuse (D53), version-validated options (D50) | re-read through the same read path |
| `gsettings` | self-contained GVDB schema/dconf parse | **native** libgio API (D51): typed set coerced to the schema's variant type + range check, real `g_settings_reset`, desktop gate fail-closed | native readback after `g_settings_sync()` |
| `dbus` (PowerProfiles) | zbus blocking client, passive name probing | typed `Properties.Set` (D52), honest `Unknown` persistence | property readback through the same endpoint |
| `mock` | declared defaults | in-memory (unchanged) | store readback |

Live-state extras: the GSettings read path is mirrored by a **native change
watcher** (gio `changed` signals on a dedicated thread) feeding the same
debounced refresh pass as the filesystem watcher; Alacritty rows surface
the detected version in Source details; the PowerProfiles reader surfaces
`PerformanceDegraded` and the profile-holds count.
