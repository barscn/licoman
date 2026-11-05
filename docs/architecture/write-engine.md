# The Safe Write Engine (Phase 06)

**Status:** implemented, fixture-only. Live writes are gated to Phase 07.

## What it is

`crates/core/src/write/` is the UI-independent mutation pipeline. The UI
never writes anything itself: it dispatches typed commands, the store turns
them into [`ChangeIntent`]s, the module provider resolves them into
[`ChangeRequest`]s, and the engine executes them through a backend family.

```text
UI command (Edit / ApplyAll / ConfirmApply / Undo / ResetToDefault)
  → Store: validate + phase transitions + ChangeIntent (no I/O)
  → adapter: background write pass (never the UI thread)
  → ModuleProvider::write_intent: resolve binding, derive policy
  → WriteEngine::apply:
       policy gate (read-only/privileged)  → refuse
       value gate (type/range)             → refuse before I/O
       target dispatch:
         File    → FileWriteBackend
         Service → ServiceWriteBackend (fake client this phase)
         GSettings → refused (Phase 07 gate)
         Mock    → provider in-memory source
  → Store::publish_apply: phase mapping — no "Applied" without a
    verification that matched
```

## The contract types

- **`ChangeIntent`** — what the user asked: setting id, the before-value as
  last read, the change (`Set(typed value)` / `RemoveOverride`), the
  expected source fingerprint. Built by the store; no paths, no commands.
- **`ChangeRequest`** — the resolved engine input: structured `WriteTarget`
  (mock / file key / GSettings key / service property), before/after typed
  values, fingerprint, risk, derived `WritePolicy`, restart effect,
  `RollbackStrategy`. Data only — there is no field that could carry shell
  text or a command string.
- **`WritePolicy`** — `read_only | immediate | staged | confirmation |
  privileged`. **Derived by the core** from the descriptor + manifest risk +
  authorization metadata; a module cannot weaken it. Any authorization
  requirement ⇒ `privileged`; high risk ⇒ at least `confirmation`.
- **`Persistence`** — `persistent | runtime_only | unknown`, reported by the
  backend and shown verbatim ("runtime only — a restart reverts it").
- **`ApplyOutcome`** — verified flag, persistence, restart, readback
  mismatch, notes. `verified` is only true when the backend read the result
  back and it matched.

## File writes (fixture roots)

`FileWriteBackend` owns a `FileReadBackend` and therefore the *same*
approved-root logic that reads use — resolution can never drift between
read and write:

1. Resolve the target through the read backend (search paths, import merge,
   canonical containment). Reject: symlink escapes, a symlink at the target
   path (refused with an explanation), non-regular files, files not owned
   by the current user.
2. Re-read and compare revision (fingerprint) **and** the expected value.
   Mismatch ⇒ `Conflict`, nothing written. Imported winners are refused —
   Licoman never edits an import it does not own.
3. Patch the minimum key(s) with `toml_edit`; the edited key's trailing
   comment is preserved; new sections render as ordinary `[table]` headers.
4. Back up **only the affected file** (`0700` dir / `0600` file, retention
   10) before staging.
5. Stage into a same-directory temp file (`O_EXCL`, `0600`), validate the
   staged document, restore the original mode, `fsync`, atomic `rename`,
   parent `fsync`. Stale temp files from a crashed process are swept on
   construction and before every staging run.
6. Read back through the same probe. Mismatch ⇒ restore the backup; a
   failed restore is reported as a failed restore with the backup path.

Platform assumptions: POSIX atomic rename within one filesystem; Unix mode
bits; ownership check via `/proc/self` (Linux-only, std-only). **ACLs and
xattrs are not preserved** by a rename-replacement and cannot be inspected
with the standard library — before Phase 07 live writes this must become
"preserve or refuse" (documented in the threat model).

## Service writes (fake client this phase)

`ServiceWriteBackend` over the `ServiceWriteClient` trait: typed
`get`/`set`/`persistence` per (endpoint, property). Every call runs on a
worker thread bounded by a timeout (injectable; bounds injected clients
too). Pipeline: pre-check the current value against the expectation ⇒
bounded typed set ⇒ readback ⇒ honest persistence. A timeout is reported as
"the service may still have applied it — refresh before retrying"; Licoman
never auto-retries. The `FakeServiceClient` keeps state per endpoint and
supports fault injection (deny, hang, change-after-apply) — the multi-
service workflow has **no global transaction**: every successful write
records its compensation (the before-value); compensation executes only on
explicit user action (`Undo`), and partial success is shown per row.

## Store integration

- `apply_begin` validates staged edits; rows whose derived policy is
  `Confirmation` stay Pending and are surfaced for the dialog; everything
  else becomes an intent for the apply pass.
- The adapter's **background write pass** (generation-guarded, like the
  read pass) executes intents on the background executor and publishes via
  `Store::publish_apply` — the single mapping from write attempts to
  phases. `Retry`, `Undo`, `ResetToDefault` and `ConfirmApply` all funnel
  through the same intents, so every write path shares the conflict check,
  the policy gate and the verification rule.
- `publish_apply` maps outcomes: verified ⇒ `Applied` (+ bounded undo
  history entry); readback mismatch ⇒ `ExternalConflict` with the source
  authoritative; `Conflict` error ⇒ row back to conflict with the edit
  retained (reload refreshes the revision); `Permission` ⇒
  `PermissionRequired`; anything else ⇒ `Failed` with a user-safe message.
- Undo is offered **only** while the recorded before-value is still known
  and the source still holds the applied value; undo itself is an inverse
  intent through the same engine path.
- Reset semantics are backend-declared and disclosed before dispatch:
  file backends delete the user's override (`RemoveOverride`), mock/service
  backends write the declared literal default.

## UI states (Phase 01 semantics, now with real outcomes)

`Pending`, `Applying`, `Applied` (+ persistence note, restart target, Undo),
`Failed` (+ Retry/Revert), `Conflict` (+ Reload/Compare — never
auto-overwrite), `Permission required` (lock + honest authorize dialog),
`Read-only`, `Unavailable`. The apply bar distinguishes "N pending" from
"N need your confirmation" and opens the confirmation dialog listing effect,
target and restart behavior.

## Test coverage

`write_matrix_tests.rs` covers the phase document's §7 matrix: external
edits, formatting preservation, import winners, symlink escape/target,
path traversal, foreign ownership (injectable uid), invalid values (no
I/O), ENOSPC during staging, crash-before-rename recovery, readback
mismatch + restore (+ failed restore honesty), service timeout/denial/
change-after-apply, two-edit partial success with undoable compensation,
privileged denial (deny-all broker), hostile text stored as data (never
executed), GSettings Phase 07 gate, bounded private backups, redacted
operation log, key removal, and absent-key insertion.

## Known limitations (honest list)

- Fixture-only: live providers carry no engine and refuse every non-mock
  write exactly as in Phase 05.
- ACL/xattr preservation unimplemented (std limitation; Phase 07 gate).
- In-root symlinks are followed on write (link preserved, canonical target
  updated) — decision D47, revisit before live writes.
- No multi-file atomicity (fundamentally unavailable; disclosed per row).
- The Applications window renders statuses but hosts no apply bar; staged
  edits apply from the main window (both windows share the store and stay
  synchronized through the same events).

## Phase 07 — the live write path (D49–D53)

The fixture-only boundary is replaced by **one engine with identical gates
everywhere** (D49): live providers carry a real engine (file over the live
approved roots with backups under `$XDG_DATA_HOME/licoman/backups`, service
over the real zbus client, GSettings over the native API); fixture engines
(demos/tests) keep injected roots and fakes. A write still requires an
attached engine + core-derived policy + engine value gate + explicit user
Apply/Confirm. `cargo test`/CI never construct a live engine.

- **GSettings writes** (`write/gsettings_write.rs`, D51): native API only —
  typed `set_value` coerced to the schema's own variant type and
  range-checked against the schema before any call; `reset` uses
  `g_settings_reset` (the platform's real reset semantics); verification
  reads back through the same native transport, after an explicit
  `g_settings_sync()` — dconf-level writes are client-queued, and without
  the sync the readback would verify the client-local view instead of the
  service (found during the live verification). A fail-closed **desktop
  gate** (fed from discovery) refuses writes outside a GNOME session even
  when schemas are installed.
- **Alacritty version gate** (`read/alacritty.rs`, D50): every option
  requires a detected TOML-era version (≥ 0.13.0); an undetectable version
  disables writes (fail-closed); legacy YAML-only setups are a distinct
  honest unavailable state. Enforced in the read path's `write_support`
  (drives the UI) and re-enforced by the engine before any I/O.
- **xattr/ACL preserve-or-refuse** (`write/file_write.rs`, D53): extended
  attributes are read before staging and re-applied to the staged file;
  any failure refuses the write before the rename with the original
  untouched. POSIX ACLs are xattrs, so this closes the Phase 06 gate.
- **PowerProfiles writes** (`read/dbus.rs` + `write/service_write.rs`,
  D52): typed `Properties.Set` on the passively probed endpoint,
  readback-verified; persistence is reported `Unknown` because the
  interface documents no persistence guarantee.

## Known limitations (updated for Phase 07)

- Live Alacritty writes require Alacritty ≥ 0.13 installed; the key
  `shell`/`working_directory` stay mock-bound (moved to `terminal.shell` in
  0.14 — one binding cannot be safe across versions).
- PowerProfiles: profile **holds** are surfaced as a diagnostics count only;
  the Hold()/Release() client API is out of scope.
- GSettings writes require a positively identified GNOME session (the gate
  is deliberately closed on unknown desktops).
- No multi-file atomicity (fundamentally unavailable; disclosed per row).
- The Applications window renders statuses but hosts no apply bar; staged
  edits apply from the main window (both windows share the store and stay
  synchronized through the same events).
