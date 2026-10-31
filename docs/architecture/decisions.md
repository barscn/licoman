# Architecture Decision Records — Phase 02

Continues the numbering from `docs/design/decisions.md` (D1–D13, Phase 01).
Each record states the decision and the reason it was taken.

## D14 — The Phase 01 mock stays as the regression reference; the repository is the migration target

**Decision:** The plan's prototype `barscn/gpui-kit-mock` was never available
on this machine (Phase 01 D1), so there is no separate prototype to preserve.
The Phase 01 implementation inside this repository *is* the working mock: its
fixtures, screenshots and tests are the regression reference. Phase 02
carried the approved UI forward by **moving** the files into
`crates/licoman-ui` (git-tracked renames) and re-expressing the mock data as
a deterministic `MockProvider` in `crates/licoman-core` with **unchanged
stable setting IDs**. Nothing was overwritten or discarded.

**Why:** the plan requires preserving the mock as a regression reference and
forbids implicit renames/overwrites; the moved files keep history, and the
Phase 01 screenshots remain comparable evidence.

## D15 — Toolchain and renderer pins are unchanged and justified

**Decision:** keep the already-working pins from Phase 01 D2:
`gpui-kit 0.8.1` (facade over `gpui`, `gpui-pre 0.3.10`, `gpui-component`),
`ashpd 0.13` with exactly `async-io` + `settings` features (mismatch fails
the build with a `compile_error!`), Rust **1.99.1 stable**, edition **2024**,
renderer **blade/Vulkan** (gpui's Linux backend), no minimum-toolchain bump
without a tested reason. The `ashpd` pin moved with the gpui dependency into
`crates/licoman-ui` so feature unification stays airtight.

**Why:** 00_START_HERE.md non-negotiable 10 forbids version bumps by
guesswork; these versions are verified working on the real session.

## D16 — Three-crate modular monolith, not five tiny crates

**Decision:** Phase 02's boundary diagram (bootstrap → UI adapter → core /
registry / provider; preferences) is implemented as **three crates**:

- `licoman-core` — domain, including the registry/provider *abstractions*
  and the mock adapter,
- `licoman-ui` — GPUI adapter,
- `licoman` (bin) — bootstrap.

The plan's suggested `crates/discovery`, `crates/backends`, `crates/safety`
crates arrive with the phases that give them substance (03, 05, 06). The
registry and provider abstractions are separate **modules** today so the
phase-04 registry implementation can be lifted into its own crate without
churn if it ever needs to be.

**Why:** 00_START_HERE.md: "A focused modular monolith is preferred";
needless tiny crates would only add indirection before real backends exist.

## D17 — Provider trait shape: probe / descriptors / read / write + capabilities; no watch in Phase 02

**Decision:** `trait Provider { id, probe, descriptors, read, write }` with
`Capabilities { read, write, watch }` returned by `probe`. `write` is
optional in effect: the store consults capabilities and refuses to call it on
read-only/permission-held settings. A `watch` method is deliberately absent;
`Capabilities::watch` exists so later phases can advertise watch support
without reshaping the trait.

**Why:** 02_FOUNDATION.md §2 lists probe/read/optional write/optional
watch/capabilities. Watch without real backends would mean polling — which
the phase explicitly forbids ("no background daemon, no idle polling").
Fail-closed: a missing capability is an explicit error, never a guess.

## D18 — The store is synchronous and UI-timing-free; the adapter owns the visible interval

**Decision:** every store operation is synchronous and deterministic:
`apply_begin` validates and marks rows Applying; `apply_complete` writes,
reads back, and settles rows in Applied/Failed/ExternalConflict. The gpui
adapter schedules the 700 ms visible interval between them and an 1800 ms
"Saved" acknowledgment afterwards. Immediate-mode settings (`ApplyMode::
Immediate` metadata) validate+write+readback synchronously inside the edit.

**Why:** core stays headless-testable (the full lifecycle is unit tested
without a display); the UI still shows truthful transient states; no timers
exist while idle (only during an apply/acknowledgment), so there is no
unexplained idle CPU activity.

## D19 — External changes are detected at load verification and on reload, not polled

**Decision:** `Store::load` ends with a verification re-read; a mismatch
becomes `ExternalConflict` (stored value kept, source value attached as
`external`). `Command::Reload` reconciles by accepting the source value (the
source stays authoritative). The mock provider simulates "the source changed"
with a deterministic read sequence instead of a timer.

**Why:** honest stand-in for the watch that arrives with real backends;
zero idle work; the conflict UI (Reload/Compare) is exercised for real.

## D20 — Validation happens once, at apply; dirty rows are unvalidated by design

**Decision:** `Edit` stores the staged value and marks the row Dirty without
judging it. `apply_begin` runs descriptor validation (type, choice options,
numeric bounds): valid rows advance Dirty → Validating → Pending → Applying;
invalid rows become Failed with a user-safe validation message plus technical
detail, and the stored value is untouched. Immediate-mode settings validate
inside the synchronous edit. This matches the phase diagram exactly
(Editing → Validating → Pending | Failure).

**Why:** "never equate a control value changing visually with a setting being
persisted" — and the diagram's validation step needs to be a real transition,
testable via the invalid-value test.

## D21 — Licoman preferences live in core, physically separate from the settings registry

**Decision:** `licoman_core::prefs` (moved verbatim from Phase 01) stays the
only code that writes to disk, and only to
`$XDG_CONFIG_HOME/licoman/preferences.toml`. Preferences are **not** entries
in the provider registry: `Command::SetAppearance` flows through the same
typed-command path but mutates only Licoman's own file, and is documented as
the safe immediate write that is distinct from provider-backed setting edits.

**Why:** 02_FOUNDATION.md §3/§4 — "keep preferences separate from the
registry and application settings"; the theme preference must remain a
first-class, clearly-bounded flow rather than a disguised provider setting.

## D22 — Hand-rolled CLI and structured logger instead of new dependencies

**Decision:** `--help`/`--version` are parsed by a small pure module
(`apps/licoman/src/cli.rs`, unit tested). Logging is
`licoman_core::log`: level-filtered (`LICOMAN_LOG`, default `warn`),
key=value records on stderr with a dependency-free RFC 3339 UTC timestamp,
and a hard privacy rule — setting **values, file contents and credentials are
never logged** (ids, counts and error kinds are).

**Why:** two flags do not justify `clap`; a value-dumping log line is the
security risk the phase calls out. Both modules are fully unit tested.

## D23 — Window single-instance and navigation history are modeled in core

**Decision:** `licoman_core::window_sessions::WindowSessions` decides
open-vs-focus per `WindowRole` and keeps an independent navigation history
per window; the gpui `WindowRegistry` stores the actual handles and delegates
every decision to it. A stale handle (window closed out-of-band) is detected
by the failed focus attempt and corrected before reopening.

**Why:** 02_FOUNDATION.md tests §3 must be testable headless — the behavior
is asserted in unit tests while GPUI only supplies handles.

## D24 — Unavailable settings show "Unavailable", not a remembered value

**Decision:** rows whose provider read fails render an explicit
"Unavailable" control value plus the provider's reason (the Phase 01 mock
displayed the last sample value instead — e.g. "Balanced" for the absent
power-profiles-daemon). Readable-but-not-writable rows keep their value and
show the provider note explaining why.

**Why:** fail-closed (00_START_HERE.md non-negotiable 4): displaying a value
Licoman cannot read would be pretending. Visual delta from Phase 01 is
intentional and limited to truly unreadable rows.

---

# Architecture Decision Records — Phase 03

Continues the numbering above (D1–D13 in `docs/design/decisions.md`,
D14–D24 in Phase 02). These records cover
`03_ENVIRONMENT_DISCOVERY.md`.

## D25 — Discovery probes are hand-rolled; `licoman-core` stays dependency-free

**Decision:** the D-Bus probe (SASL EXTERNAL + `Hello` + `ListNames`) and the
GVDB reader for `gschemas.compiled` are implemented in
`crates/core/src/discovery/` with **zero new dependencies** in the core
crate. The `gsettings` CLI and any client library (zbus et al.) are
deliberately not used: `gsettings` can auto-activate `dconf-service`, and the
plan forbids launching daemons solely to enable a module; a client library
would pull a large async tree into the UI-independent core. Both hand-rolled
pieces are exact-byte/real-file tested and degrade gracefully
(unavailable + problem entry) on any failure.

**Why:** continues D22's direction (hand-rolled small surface over big
dependency trees); the probe surface is two no-argument D-Bus calls and a
read-only GVDB walk — small enough to own, and every byte is explainable.

## D26 — Filesystem watching lives in the UI adapter; `notify` is the one new dependency of Phase 03

**Decision:** the plan requires filesystem notifications with debouncing for
cache invalidation. The `notify` crate (inotify on Linux) is added to
`licoman-ui` only — the adapter already owns all timing (D18) — and
`licoman-core` gains nothing. The watcher is three dumb pieces: notify's
thread → a 500 ms debounce thread → a background-executor task that
refreshes discovery on the main thread. The blocking receive is
event-driven (zero wakeups at idle) and the whole thing is skipped for demo
fixtures.

**Why:** std has no inotify; low-frequency polling is explicitly discouraged
by the plan; keeping the dependency out of core preserves the Phase 02
structural guarantee that the domain crate depends on nothing GUI- or
OS-event-shaped.

## D27 — The discovery cache belongs to the adapter, not the `Store`

**Decision:** `EnvironmentSnapshot` is cached in `AppState`'s
`EnvironmentState` (adapter), not in `licoman_core::Store`. `Store` keeps its
D18 contract: synchronous, UI-timing-free, no environment I/O. The adapter
publishes partial snapshots after every stage and re-derives the
app/module merge (`build_app_list`) only on ingestion.

**Why:** the snapshot is an observation of the environment, not settings
state; mixing it into the store would entangle load/dispatch semantics with
async discovery. The five support conditions (installed / module available /
version compatible / backend available / writable) stay explicit fields, so
the UI can show each honestly.

## D28 — `Command::SelectApp` carries a stable id, not an index

**Decision:** `SelectApp { index: Option<usize> }` becomes
`SelectApp { id: Option<String> }` (desktop-file id or module id). The
Applications list is now dynamic — it changes between scans as applications
are installed/removed — so indices would silently point at the wrong row
after a refresh. Selection survives refreshes by identity.

**Why:** same lesson as the search index (Phase 02): identities are stable,
indices are not.

## D29 — Demo fixtures run through the real parsers

**Decision:** `LICOMAN_DEMO` fixture names (`arch-hyprland`, `fedora-gnome`,
`ubuntu-x11`, `debian-xfce`, `multi-xdg`, `unknown`) build
`EnvironmentSnapshot`s by running the actual discovery parsers over embedded
fixture data — never by hand-writing display strings. Demo mode locks out
live discovery and the filesystem watcher.

**Why:** demo screens must verify the rendering of actual parser output for
environments this machine cannot run (different distros, X11, ambiguous
sessions); hand-built snapshots would rot and prove nothing about the
parsers.

## D30 — Diagnostics are a CLI export, not a window

**Decision:** `licoman --diagnostics` prints the redacted discovery export
headless and exits 0 (no graphical session required). The export carries
ids, counts, statuses, evidence and problems; `$HOME` paths are redacted to
`~`; `Exec` lines, icon payloads, file contents and setting values are never
included.

**Why:** the plan asks for "a diagnostics view or export"; the export is
testable, scriptable, and needs no extra window. UI integration can build on
the same function later.

## D31 — Built-in manifests are embedded at build time

**Decision:** the built-in module manifests (`modules/builtin/*.toml`) are
compiled into the binary via `include_str!` and listed in an explicit table
(`crates/core/src/modules/builtin.rs`). There is no runtime module search
root, so no file can override a built-in module and "source precedence"
degenerates to a trivial, checkable rule: build-time content only. External
data-only module loading is deferred until the separate security review the
plan demands; the loader is generic over `(source, text)` pairs so the
read-only linter and tests exercise the same code path without the app ever
opening untrusted paths.

**Why:** 04_MODULE_SCHEMA_AND_REGISTRY.md requires built-ins "trusted at
build/release time" and forbids silent overrides by same-named files.
Embedding is the strongest form of both, needs no trust checks at runtime,
and keeps the hard boundary (no manifest may execute or write) structural.

## D32 — Descriptors become runtime data

**Decision:** `SettingDescriptor` text fields change from `&'static str` to
owned `String` (and `Control` payloads likewise), gaining `keywords` (search)
and `module_id` (provenance). `Scope` gains an `Application` variant that the
main-window sidebar never offers (it renders only in the Applications
window), and `Control` gains an inert `Action` variant.

**Why:** descriptors now come from validated manifests, so static strings
cannot work. The conversion keeps the Phase 02 store/command/rendering
machinery unchanged — the module registry feeds the same interfaces rather
than forking a second settings pipeline.

## D33 — The module registry reaches the store through a `Provider`

**Decision:** `modules::ModuleProvider` implements the existing `Provider`
trait over a `Mutex<ModuleRegistry>` (`Arc`-cloned: one handle in
`AppState`, one behind the `Provider` object). Applicability changes arrive
via `set_environment(snapshot)`; the store reloads **only when the set of
available settings changed**, preserving navigation and staged edits
otherwise. Mock values are served only from declared defaults; non-mock
backends fail closed on read ("not readable yet") and unconditionally on
write.

**Why:** the plan's lifecycle ends in "provide normalized data to GPUI
adapter" without duplicating state. The provider bridge reuses the store's
load/verify/dirty/apply semantics (and its tests), and the conditional
reload honors "preserve App/Setting navigation when a module is refreshed".

## D34 — Applicability reasons are distinct, preserved data

**Decision:** every incompatibility dimension (desktop, distro,
application/version, backend capability, per-setting predicate, missing
schema/key/service) produces its own `IncompatibilityReason` with a
user-safe message; a pending environment is its own state, never a guess.
A declared app-version requirement is conservatively inapplicable while
versions cannot be detected.

**Why:** the plan's test matrix distinguishes schema-version, app-version,
DE, service, permission and missing-binary failures; collapsing them into
one boolean would make the UI lie by omission.

## D35 — The JSON Schema is generated from the canonical types

**Decision:** the definition types derive `schemars::JsonSchema`; 
`licoman module schema` prints the schema. `schemars` (pure Rust, no I/O) is
the only new core dependency of this phase.

**Why:** 04 §5 requires a machine-validatable contract "generated from
canonical Rust types" — hand-maintained copies would drift.

## D36 — Mock value binding rules

**Decision:** `mock`-backed settings must declare a default (validation
error otherwise) — the mock source never invents values — except
`string`/`path` settings, where an empty value is the explicit "not set".
Nullable is restricted to string/path. Writes are refused for `read_only`
policy, for `required_auth` settings, and unconditionally for non-mock
backends. One deterministic fixture id (`shared.network.dns`) fails mock
writes to keep the failure flow demonstrable.

**Why:** "Represent absence correctly" and "not silently assigned defaults"
(04 §3) become validation rules instead of runtime conventions.

## D37 — Settings-group metadata drives the Applications-window tabs

**Decision:** `group` lives on the *setting* (falling back to module
`group`, then `category`), while the plan's module-level `category` groups
rows in the main-window scope pages. `order` exists on both module and
setting and is stamped explicitly by the catalog so display order equals
declaration order.

**Why:** the plan requires "registry app definitions provide its list,
category tabs and detail panels"; per-setting groups are the smallest
honest way to derive tabs, and explicit orders keep grouping deterministic.

## D38 — The typed read layer lives in `licoman_core::read`, one envelope per read

**Decision:** Phase 05's read API is `crates/core/src/read/`: a
`SettingsReadBackend` trait (`probe` / bounded `read` / `watch_paths`) and a
`ReadResult` envelope — typed `SettingValue` plus `ValueOrigin`
(default/system/user/managed/runtime/unknown), `Writable` (fail-closed:
real backends report `No` in this phase), `Freshness` (read time + FNV-1a
fingerprint over the source bytes), `RestartEffect`, and `SourceDetails`
(metadata only, never contents). A `BackendRouter` maps a validated
`BackendBinding` to its backend family. The `Provider` trait gains
`read_detailed` and a `read_latency` hint (Immediate = mock, Background =
real I/O); the store defers Background rows to a generation-guarded
background read pass and publishes per row.

**Why:** 05 §1 demands a typed API with provenance rather than per-module
subprocesses; the latency hint is what keeps real I/O off the UI thread
without reshaping the synchronous store (05 §2: one backend failing must
not block the UI).

## D39 — GSettings is read from the platform's own databases, no subprocess

**Decision:** the GSettings backend parses the same GVDB files the platform
uses: `gschemas.compiled` (schema presence, `.path` attribute, and the
default value decoded from GLib's framed *non-normal* GVariant form) and
dconf databases (user db first — highest priority — then the system
databases listed in the dconf profile). Origin mapping: user db → `User`,
system db → `Managed`, otherwise the schema default → `Default`. A
relocatable schema (no `.path`) reads defaults-only with a disclosed
degraded note; `GSETTINGS_BACKEND` other than dconf adds a diagnostic.
No `gsettings` CLI, no shell, no library dependency: the existing
Phase 03 GVDB parser was extended (`lookup_schema_value`,
`lookup_schema_path`, `has_schema`, `dconf_lookup`) with a bounded
non-normal GVariant decoder (`read::gvariant`) verified byte-for-byte
against a real `~/.config/dconf/user`, a `dconf compile` scratch db, and
the system `gschemas.compiled`.

**Why:** 05 §2.A prefers native APIs and forbids shells; the on-disk
databases *are* the native storage, and the formats are stable, bounded,
and fully testable offline. Arrays/dictionaries are an explicit
unsupported-type error, never a guess.

## D40 — Alacritty TOML: documented search paths plus the documented import merge

**Decision:** the file backend implements the official Alacritty
resolution: candidates `$XDG_CONFIG_HOME/alacritty/alacritty.toml`,
`$XDG_CONFIG_HOME/alacritty.toml`, `$HOME/.config/alacritty/alacritty.toml`,
`$HOME/.alacritty.toml`, `/etc/alacritty/alacritty.toml` (first existing
wins as root), and `general.import` semantics — "Imports are loaded in
order, skipping all missing files, with the importing file being loaded
last" / "If a field is already present in a previous import, it will be
replaced" — so the merge order is [imports…, root] and the last loader
wins. Per-setting provenance records which file won and whether it was an
import. Bounds: 256 KiB/file, import depth ≤ 8, ≤ 32 files, cycle
detection along the current chain (duplicate imports are allowed; true
cycles are skipped with a diagnostic), invalid TOML/UTF-8 are distinct
errors, and every canonicalized path must stay inside the authorized
roots (config home, home, `/etc/alacritty`) — symlink escapes are
rejected, not followed. `toml_edit` parses (the later write phase needs
formatting preservation). Missing config is a normal state: no value is
invented because Licoman ships no validated built-in defaults. Non-TOML
file bindings (e.g. Hyprland keyfile) stay recorded-but-unread.

**Why:** 05 §2.B requires real documented precedence; a parsed first file
must never be shown as the effective config, and the security rules cap
imports, sizes and traversal (05 §4).

## D41 — Power profiles over zbus with a bounded client boundary

**Decision:** the D-Bus backend uses `zbus` 5 (blocking API, async-io
feature set matching the ashpd already in the tree) behind a
`PowerProfilesClient` trait; the backend bounds *every* client call with a
worker thread + 3 s timeout, so an unresponsive service — real or
injected — becomes a `Timeout` error instead of a hang. Name probing is
passive `ListNames` over both endpoints (`org.freedesktop.UPower.PowerProfiles`
and legacy `net.hadess.PowerProfiles`); absent service, unreachable bus,
timeout and denied replies are distinct states. `ActiveProfile` and
`Profiles` read as runtime values (`ValueOrigin::Runtime`). Discovery now
also probes the **system bus** names (the session-bus-only applicability
check would never see a system service); no service is ever activated.

**Why:** 05 §2.C names zbus and demands absence/connect-failure distinction
and no polling; the trait boundary keeps the whole §6.3 fixture matrix
testable without a daemon on CI.

## D42 — Phase 05 is read-only end to end

**Decision:** every real backend reports `Writable::No` ("… cannot be
changed yet"); the UI renders non-writable rows through the read-only path
(value + explicit note, disabled controls, no entities built), writes to
real backends are refused unconditionally (unchanged from Phase 04), and
`licoman_core` gained `toml_edit` + `zbus` as its only new dependencies.

**Why:** the plan's stop condition — "Do not enable real writes merely
because a read was successful" — is enforced by the envelope, not by
convention.

## D43 — The write policy is core-derived, never module-declared

**Decision:** the effective `WritePolicy` (`read_only | immediate | staged |
confirmation | privileged`) is computed by the core
(`write::derive_write_policy`) from the validated descriptor plus the
module's risk/auth metadata: any authorization requirement ⇒ `privileged`;
read-only controls ⇒ `read_only`; high declared risk ⇒ at least
`confirmation`; otherwise the descriptor's apply mode. Both the store and
the engine enforce the derived policy (defense in depth). A new
`SettingPhase::PermissionRequired` gives denied rows a distinct, truthful
state with the honest authorize dialog.

**Why:** 06 §1 — "A module cannot arbitrarily mark itself safe and bypass
enforcement. Backend + core own the policy." A manifest field that decided
the policy would be a trust hole; derivation makes the conservative choice
unconditional.

## D44 — Fixture-only writes through an explicit engine attachment

**Decision:** `ModuleProvider` carries an `Option<Arc<WriteEngine>>`. `None`
(live providers) keeps the Phase 05 hard boundary: every non-mock write is
refused. The engine attaches only via `with_fixture_write_engine`, which
also flips the registry into `fixture_writes` mode (descriptors keep their
natural controls, read envelopes report `Writable::Yes` for file/dbus
families). GSettings stays fail-closed even in fixture mode — the engine
refuses it with a Phase 07 gate. Demos copy committed fixtures into a temp
tree and delete it on exit; no live environment ever has an engine.

**Why:** 06's hard boundary ("No actual external application, DE,
distribution or system writes in Phase 06") must be enforced by
construction, not by discipline; a provider *without* an engine cannot
write, whatever calls it.

## D45 — Writes run off the UI thread via a generation-guarded write pass

**Decision:** immediate edits, staged applies, retry, undo, reset and
confirm all end in stashed `ChangeIntent`s + rows in `Applying`; the
adapter's write pass (visible 500 ms interval first, then background
executor, generation-guarded like the Phase 05 read pass) executes them
through `Provider::write_intent` and publishes via the single
`Store::publish_apply` mapper. The synchronous `Store::apply_complete`
remains for tests and mock-only flows and uses the same mapper.

**Why:** Phase 05 established "no I/O on the UI thread"; real file/service
writes would have violated it. One mapper for both execution paths means
the phase rule — "UI never reports Applied before the backend verifies the
result" — has exactly one implementation.

## D46 — Honesty rules for service outcomes and multi-setting workflows

**Decision:** service writes verify by typed readback; a mismatch is an
`ExternalConflict` with the service authoritative, never an "Applied".
Persistence is reported as `persistent | runtime_only | unknown` and shown
verbatim. Timeouts say the change *may* have been applied and never
auto-retry. There is no global transaction across settings: each successful
write records its compensation (the before-value, bounded undo history);
compensation executes only on explicit user action, and partial success is
rendered per row.

**Why:** 06 §3 — "A successful D-Bus reply does not necessarily prove
persistence" and "no global atomic transaction … best-effort compensation
log and clear partial-success UI instead of claiming all-or-nothing
semantics."

## D47 — In-root symlinks are followed on write; the link is preserved

**Decision:** the write target is always the **canonical** path inside the
approved roots. A symlink at the requested path that resolves inside the
roots is therefore followed: the canonical file is updated through the
normal backup/stage/rename discipline and the user's link itself is
preserved (never replaced by a regular file). Links resolving **outside**
the roots are refused by containment before anything is read or written.

**Why:** refusing in-root links outright would break the common
managed-dotfile pattern (config paths linking into a dotfiles directory);
writing through them is bounded by the same containment checks as any other
write. Residual risk (Licoman may modify a file reachable through an
approved-root link) is recorded in `docs/security/threat-model.md` and must
be revisited before Phase 07 live writes.

## D48 — Backups, undo and reset are bounded and disclosed

**Decision:** every modifying file write backs up only the affected file
under `<data-root>/backups/<setting>/` (`0700`/`0600`, retention 10, pruned
oldest-first). Undo history in the store is bounded (32) and per-setting;
undo is offered only while the original value is known and still absent
from the source being overwritten — and undo is an inverse intent through
the same engine path, so it inherits conflict checks and backups. Reset
semantics are backend-declared: file backends delete the user's override
(disclosed as such), mock/service backends write the declared literal
default (also disclosed). The engine's operation log records ids, backend
kind, redacted target, outcome and elapsed time — bounded at 100, never
values or contents (asserted by a planted-secret test).

**Why:** 06 §2.7 (bounded, private, target-only backups), §5 (undo only
when actually possible; reset must disclose), §6 (bounded redacted
history).
