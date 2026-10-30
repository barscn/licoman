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
