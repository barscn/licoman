# Phase 06 Report — Safe Configuration Writes, Validation and Recovery

**Date:** 2026-10-31
**Workspace:** `/home/baris/Work/licoman` (`main`, Phase 05 commit + this
phase's uncommitted changes — nothing was committed or pushed by the
automated session, per the execution contract; the tree is ready for review).
**Executor:** automated session (ZCode), one phase per session as required.
**Plan source:** `06_SAFE_WRITE_ENGINE.md` (author's document pack).
All nine section headings of the required handoff format are below.

---

## 1. Goal and actual scope implemented

**Goal (from the plan):** a robust, auditable, UI-independent write
pipeline **using isolated fixtures and fake services only**, with the
safety model fully in place before any live setting is enabled. Honored
end to end: the hard boundary is structural — live providers carry **no
engine** and refuse every non-mock write exactly as in Phase 05; the write
engine exists only when explicitly constructed over injected fixture
roots (the demos), which copy committed fixtures into a throwaway temp
tree deleted on exit. **No real host file, dconf database or service was
touched by implementation or tests.**

### The typed mutation contract (`crates/core/src/write/`, new, 7 files)

- **`ChangeIntent` → `ChangeRequest`** (request.rs): the user's ask
  (setting id, before-value, `Set(typed)` / `RemoveOverride`, expected
  fingerprint) resolves into the engine input (structured `WriteTarget`,
  policy, risk, restart effect, rollback strategy). Data only — no field
  can carry shell text or a command string (D43's grammar guarantees hold
  at the type level).
- **Core-derived policies** (mod.rs): `read_only | immediate | staged |
  confirmation | privileged` derived from the descriptor + manifest
  risk/auth — a module cannot weaken them, and both the store and the
  engine enforce the derivation (D43). New `SettingPhase::PermissionRequired`
  gives denied rows a distinct state with the honest authorize dialog.
- **`PrivilegeBroker`** (broker.rs): deny-by-default (`DenyAllBroker`) is
  the only implementation; the future polkit/allowlisted-helper design is
  documented in `docs/security/threat-model.md`, none of it is built.
- **`WriteEngine`** (engine.rs): policy gate → value gate (type/range
  **before any I/O**) → target dispatch (file / service / mock / refused
  GSettings) → bounded redacted operation log (ids, backend, redacted
  target, outcome, elapsed; 100 entries; never values — asserted by a
  planted-secret test).
- **`FileWriteBackend`** (file_write.rs): resolution through the read
  backend's own approved-root logic (containment, import merge), hostile-
  shape rejection (symlink escapes and symlink targets, non-regular files,
  foreign ownership via an injectable uid check), fingerprint **and**
  semantic conflict checks, imported-winner refusal, `toml_edit` minimum-
  key patches (comments/ordering/unrelated keys preserved — the edited
  key's trailing comment too), bounded private backups (`0700`/`0600`,
  retention 10), `O_EXCL` `0600` staging on the same filesystem, mode
  preservation, `fsync` + atomic `rename` + parent `fsync`, crash-recovery
  temp sweeps, readback verification with backup-restore on mismatch and
  honest reporting when restore fails, and the explicit I/O error taxonomy
  (ENOSPC/EACCES/lost files/read-only fs).
- **`ServiceWriteBackend` + `FakeServiceClient`** (service_write.rs): typed
  get/set/persistence per (endpoint, property), every call bounded by a
  worker-thread timeout (injectable), pre-check ⇒ set ⇒ readback, honest
  `persistent | runtime_only | unknown` persistence, no automatic retry of
  a timed-out mutation, fault injection (deny / hang / change-after-apply).
  The fake also implements the read-side client so demos and tests use one
  service for both.

### Store and adapter integration

- `apply_begin` validates staged edits; rows whose derived policy is
  `Confirmation` stay Pending and are surfaced for the dialog
  (`store.confirmation_pending()`); everything else becomes an intent.
  The adapter's **generation-guarded background write pass** (the Phase 05
  read-pass pattern) executes intents on the background executor and
  publishes via the single `Store::publish_apply` mapper — no I/O on the
  UI thread (D45). Immediate edits, Retry, Undo, ResetToDefault and
  ConfirmApply all funnel through the same intents, so every write path
  shares conflict checks, policy gates and the verification rule.
- **No false success:** "Applied" requires a verification read that
  matched the request (engine-verified or store readback). A readback
  mismatch is an `ExternalConflict` with the source authoritative; a
  `Conflict` error keeps the edit and the row in conflict (Reload
  refreshes the revision — including the stashed intent's); `Permission`
  lands in `PermissionRequired`; anything else is `Failed` with a
  user-safe message. Persistence is shown verbatim ("runtime only — a
  restart reverts it").
- **Undo** appears only while the recorded before-value is known and the
  source still holds the applied value; undo is an inverse intent through
  the same engine path (conflict-checked, backed up, verified). History
  bounded at 32, per setting. **Reset** semantics are backend-declared and
  disclosed in a dialog before dispatch: file backends delete the user's
  override (`RemoveOverride`), mock/service backends write the declared
  literal default (D48).

### UI (both windows)

- Status lines gained `Permission required` (lock + Authorize… → honest
  alert), "Saved — runtime only…" persistence notes, **Undo** on freshly
  applied rows, and **Reset to default…** with the disclosure dialog
  (deletes-override vs writes-literal, plus the conflict-check/backup
  note) on editable Idle rows. The apply bar distinguishes "N pending"
  from "N need your confirmation" and the **confirmation dialog** lists
  effect, target provenance and restart behavior per row before dispatching
  `ConfirmApply` (core-enforced policy; the dialog is the explicit yes).
- Conflict rows keep the existing Reload + Compare dialog (never an
  overwrite button); the source stays authoritative.
- Both windows share the store and the same events; a control-set rebuild
  triggers when the background read pass completes (rows become
  interactive when the envelope flips to writable in fixture mode). The
  Applications window renders the same statuses; it hosts no apply bar —
  staged edits apply from the main window (documented limitation).

### Demo hooks and theme verification

`LICOMAN_DEMO=writes | writes-conflict | writes-permission | writes-light`
build the fixture write environment (temp copy of `tests/fixtures/
alacritty` + fake PowerProfiles service with seeded `ActiveProfile` /
`Profiles`) and stage deterministic states: two pending edits (service +
file) with the apply bar; a staged row whose service value changed
externally behind Licoman's back (the fake daemon genuinely mutates, the
row shows the conflict Reload/Compare state); the permission-held row in
the distinct Permission-required state; and the Light theme over the same
write states. Theme persistence: first-launch Dark and mode persistence
are core-tested (`defaults_are_dark`, `valid_file_roundtrips`); the
light-while-dialog visual check rides `writes-light` (capture deferred,
§5). The smoke run verified the flow headless: 28 fixture-backed reads
published (file rows `User`, fake-service rows `Runtime`), both edits
staged at the 4 s mark.

## 2. Changed/added files and public interfaces

```text
crates/core/src/write/            NEW (7 files, ~3.6k lines incl. tests):
  mod.rs            WritePolicy + derive_write_policy, Persistence,
                    ApplyOutcome, OperationRecord/OperationLog (bounded,
                    redacted), io error taxonomy, conflict() constructor
  request.rs        ChangeIntent, Change/ChangeRequest, WriteTarget,
                    ServiceEndpoint/ServiceBus, RollbackStrategy,
                    ResetDisclosure/ResetSupport
  broker.rs         PrivilegeBroker trait + DenyAllBroker (deny-by-default)
  file_write.rs     FileWriteBackend (conflict checks, toml_edit patch,
                    backup, atomic staging, fault injection, temp sweep)
  service_write.rs  ServiceWriteClient trait + FakeServiceClient +
                    ServiceWriteBackend (typed, bounded, honest)
  engine.rs         WriteEngine (gates, dispatch, redacted op log)
  write_matrix_tests.rs  §7 adversarial matrix (20 scenarios) + full-stack test
crates/core/src/state.rs          + SettingPhase::PermissionRequired
crates/core/src/command.rs        + Undo / ResetToDefault / ConfirmApply
crates/core/src/model.rs          ChangeIntent plumbing, publish_apply (the
                    single outcome→phase mapper), confirmation holds,
                    bounded undo history, PermissionRequired/Immediate/
                    Undo/Reset/Confirm paths; reload refreshes provenance
crates/core/src/provider.rs       + write_intent (default = legacy write +
                    best-effort readback), write_risk, reset_support,
                    WriteOutcome (outcome + verification read)
crates/core/src/registry.rs       + write_intent / write_risk / reset_support
crates/core/src/modules/provider.rs  engine attachment
                    (with_fixture_write_engine), write_intent override,
                    writable=Yes for engine-backed families (gsettings
                    stays No), manifest risk, reset support per backend
crates/core/src/modules/registry.rs  fixture_writes flag
                    (enable_fixture_writes; descriptors keep natural
                    controls in demo mode only)
crates/core/src/read/file.rs      + FileKeyProbe/probe_key (structural key
                    resolution: winner, imported flag, value or absent,
                    fingerprint) — read() now defined on top of it;
                    + redact_path; Display for ClientError (dbus.rs);
                    ENDPOINTS pub(crate)
crates/ui/src/app_state.rs        background write pass (kick_apply/
                    run_apply_pass, generation-guarded) replacing the
                    old timer-completed sync apply; ack scheduling
crates/ui/src/ui/main_window.rs   PermissionRequired arm (lock+authorize),
                    Undo on Applied, Reset-to-default disclosure dialog,
                    confirmation dialog + apply-bar states
crates/ui/src/ui/setting_controls.rs  PermissionRequired status text
crates/ui/src/ui/apps_window.rs   rebuild on read-pass completion (rows
                    become interactive); had_pending_reads tracking
apps/licoman/src/writes_demo.rs   NEW: temp fixture env + fake service +
                    engine construction, external-edit injection, cleanup
apps/licoman/src/main.rs          writes/writes-conflict/writes-permission/
                    writes-light demo wiring
apps/licoman/src/cli.rs           help: Phase 06 demos + honest write note
tests/tools/capture-p06.sh        NEW: screenshot harness (see §5 note)
docs/security/threat-model.md     NEW (required deliverable)
docs/architecture/write-engine.md NEW (required deliverable)
docs/architecture/decisions.md    D43–D48
docs/architecture/README.md       status + write-engine note
docs/module-schema.md             Phase 06 policy-enforcement semantics
00_START_HERE.md, README.md       status banners + demo table
docs/handoffs/PHASE_06_REPORT.md  this file
```

Key public interfaces:

- `licoman_core::write`: `ChangeIntent`, `Change`, `ChangeRequest::resolve`,
  `WriteTarget`, `ServiceEndpoint`/`ServiceBus`, `WritePolicy` +
  `derive_write_policy`, `Persistence`, `ApplyOutcome`, `OperationRecord`/
  `OperationLog`, `PrivilegeBroker`/`DenyAllBroker`, `WriteEngine`
  (`apply`, `operations`, `broker_id`), `FileWriteBackend` (+ `FaultSet`,
  `with_uid`, `sweep_stale_temps`), `ServiceWriteBackend`,
  `ServiceWriteClient`/`FakeServiceClient`/`FakeFaults`,
  `ResetDisclosure`/`ResetSupport`.
- `licoman_core`: `Store::{publish_apply, take_apply_intents,
  confirmation_pending, undo_available, reset_support, write_policy}`,
  `SettingModel.persistence`, `SettingPhase::PermissionRequired`,
  `Command::{Undo, ResetToDefault, ConfirmApply}`,
  `Provider::{write_intent, write_risk, reset_support}` + `WriteOutcome`,
  `Registry::{write_intent, write_risk, reset_support}`,
  `read::file::{FileKeyProbe, FileReadBackend::probe_key/redact_path}`.
- `licoman_core::modules`: `ModuleProvider::with_fixture_write_engine`,
  `ModuleRegistry::enable_fixture_writes` (demo-only).
- `licoman_ui`: `AppState` write pass internals (private); window status/
  dialog additions are internal to the windows.
- Binary: `LICOMAN_DEMO` gains `writes`, `writes-conflict`,
  `writes-permission`, `writes-light`.
- Module schema: **unchanged** (`schema_version` stays 1); the phase
  documents how existing `policy`/`risk`/`required_auth` fields are
  enforced (no new manifest surface — policies are derived, per D43).

## 3. Decisions and deviations

Recorded in `docs/architecture/decisions.md` (D43–D48): **D43** core-
derived write policies (a manifest cannot mark itself safe);
**D44** fixture-only writes via explicit engine attachment (live providers
structurally cannot write); **D45** all writes off the UI thread through a
generation-guarded write pass with one outcome mapper; **D46** service
honesty (verified readbacks, persistence labels, no auto-retry,
compensation-log-not-auto-rollback for multi-setting workflows); **D47**
in-root symlinks followed on write with the link preserved (documented
residual risk); **D48** bounded backups/undo/history, disclosed reset
semantics.

Deviations / judgment calls (all disclosed):

- **Compensation is recorded, not auto-executed.** For a multi-setting
  workflow where a later op fails, earlier successful ops stay applied and
  the compensation (recorded before-value) executes only on explicit user
  action. The plan asks for "a best-effort compensation log and clear
  partial-success UI instead of claiming all-or-nothing semantics" — the
  log exists (undo history + engine operation log) and the partial-success
  UI is per-row; auto-rollback was judged riskier than explicit undo
  because it would silently destroy intentional changes.
- **In-root symlinks are followed on write** (canonical target updated,
  link preserved) rather than refusing all symlinks: refusing would break
  managed-dotfile layouts. Escapes are still impossible (containment).
  Residual risk documented in the threat model; revisit before Phase 07.
- **ACL/xattr preservation is NOT implemented** (std has no API). The plan
  allows "preserve when supported or refuse/explain": the phase documents
  the limitation explicitly (file_write.rs header, threat model) and
  gates it to Phase 07, where a real-write phase must implement
  preserve-or-refuse before any live write. No silent discarding claim is
  made anywhere.
- **The ownership check reads `/proc/self`'s uid** (std-only, Linux-only)
  with an injectable provider so the foreign-ownership rejection is
  actually tested; no libc dependency was added.
- **`Reload` and `Retry` now refresh the read provenance** (fingerprint)
  including stashed intents, so reconcile-then-retry re-checks the current
  revision instead of false-conflicting on a stale one.
- **Store::apply_complete remains synchronous** for tests and mock-only
  flows (it routes through the same intents + publish_apply mapper); the
  UI uses the background pass. Two execution paths, one outcome mapper.
- **The Applications window hosts no apply bar** (statuses only); staged
  edits apply from the main window — both windows share the store and stay
  synchronized through the same events (verified by design and by the
  `writes` demo staging rows across both).

**Not implemented (explicitly out of scope per the plan):** any live write
(GSettings/dconf writes refuse with a Phase 07 gate even in fixture mode);
any real privileged helper or polkit integration; package-manager
operations; multi-file atomic transactions (impossible in general;
disclosed per row instead).

## 4. Tests/commands run, with real outcomes

All commands from the repository root; none skipped. Final state:

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | **pass** |
| `cargo test --workspace` | **pass — 236/236** (core 225, cli 6, ui 5), 0 failed, 0 ignored |
| `cargo clippy --workspace --all-targets -- -D warnings` | **pass — 0 warnings** |
| `cargo build --workspace --release` | **pass** |
| `./target/release/licoman --version` | `licoman 0.1.0` |
| `./target/release/licoman --diagnostics` | unchanged redacted export (headless) |
| `./target/release/licoman module validate modules/builtin/` | 15/15 valid |
| `./target/release/licoman --help` | updated demo + write-honesty notes |
| Writes demo smoke run (release binary, `LICOMAN_LOG=info`, 6 s) | 28 fixture-backed reads published (file=User, fake-service=Runtime origins); both demo edits staged at the 4 s mark; temp env cleaned |

Test coverage mapped to the plan's **adversarial matrix** (§7) — every row
machine-checked in `crates/core/src/write/write_matrix_tests.rs` (20
scenarios; the write-module suites total 34 tests including the policy/log
unit tests):

| Scenario (plan §7) | Test | Result |
|---|---|---|
| User edits same TOML externally after read | `external_edit_after_read_conflicts_and_never_overwrites` (fingerprint AND semantic arms) | pass |
| Comments, custom ordering, unrelated keys preserved | `targeted_patch_preserves_comments_ordering_and_unrelated_keys` (byte-exact) | pass |
| Nested imports/overrides | `imported_winner_is_refused_and_root_winner_edits_the_root` | pass |
| Symlink outside root / at target | `symlink_escape_and_symlink_target_are_rejected` (in-root link preserved + escape refused) | pass |
| Path traversal from module field | `path_traversal_from_the_binding_is_rejected_before_any_file_access` | pass |
| Another user owns config file | `foreign_owned_file_is_rejected_with_permission_error` (injectable uid) | pass |
| Invalid value/type/range → no IO | engine value gate (`shell_text_in_values…` first arm) + store validation | pass |
| Disk full during staging | `disk_full_during_staging_leaves_the_original_intact` (+ debris sweep asserted) | pass |
| Process dies before rename | `crash_before_rename_is_recovered_by_the_sweep` | pass |
| Mock D-Bus times out / rejects | `service_timeout_and_denial_are_not_reported_as_applied` | pass |
| Mock service changes value after apply | `service_change_after_apply_is_reported_as_conflict_not_success` | pass |
| Two backend edits, second fails | `two_service_edits_with_second_failure_stay_partial_and_undoable` (partial success + explicit compensation via Undo) | pass |
| Backup/restore fails | `verification_failure_restores_the_backup` (+ failed-restore honesty arm) | pass |
| Privileged write requested | `privileged_request_is_denied_by_the_deny_all_broker` | pass |
| Malicious TOML manifest includes shell text | `shell_text_in_values_is_stored_verbatim_and_never_executed` (+ Phase 04 grammar) | pass |
| Secret-like value not leaked | `backups_are_bounded_private_and_redacted_in_the_log` (planted token absent from all records) | pass |
| Backup retention/permissions | same test (10 kept, `0600` asserted) | pass |
| Key removal (reset) | `remove_override_deletes_only_the_target_key_and_preserves_the_rest` | pass |
| Absent-key insertion | `setting_an_absent_key_creates_it_without_touching_other_sections` | pass |
| GSettings Phase 07 gate | `gsettings_writes_are_gated_to_phase_07_and_mock_targets_refused` | pass |
| Full stack (store→engine→file) | `full_stack_store_to_file_write_conflict_undo_and_reset` (apply, conflict, undo-refusal-after-external-change, reset) | pass |
| Policy derivation un-weakenable | `policy_derivation_cannot_be_weakened_by_the_module` (write/mod.rs) | pass |
| Bounded op log ids | `operation_log_is_bounded_and_ids_increase` | pass |
| Store-level flows | existing 205 core tests updated for the intent flow (immediate edit → Applying → complete; retry; revert incl. PermissionRequired) | pass |

**Bugs found and fixed during verification** (recorded honestly):

1. **Implicit-table rendering assumption**: toml_edit renders a freshly
   inserted table with a `[font]` header (not dotted-key form). The test
   pinned the real output; the doc comment was corrected — insertions are
   one minimal section, still formatting-preserving for existing content.
2. **ENOSPC debris**: the fault path originally left the truncated temp
   file behind ("the caller cleans it up" — nobody did). Fixed to clean
   up; the test now asserts zero debris.
3. **Sweep scope**: the constructor swept only the backup root; crash
   recovery now also sweeps the config home top level and the alacritty
   directory (the backend's known write targets), and the demo env sweeps
   stale temp trees from hard-killed instances.
4. **Dead symlink check**: the "symlink at target" check ran on the
   already-canonicalized path and could never fire; the honest semantics
   (in-root links followed, link preserved, escapes refused) are now
   implemented deliberately and tested both ways (D47).
5. **Stale revision after Reload**: `reload()` refreshed `stored` but not
   `read_meta`, so a follow-up intent (reset in the full-stack test)
   false-conflicted; Reload now uses `read_detailed` and refreshes the
   stashed intent's revision.
6. **Readback falsification risk**: an early draft mapped a failed
   verification read to "Applied" for removals; the final mapper requires
   the engine's own `verified` flag or an explicit "not present" corroboration
   (`expect_absent`), never a bare read error.

**Bugs found and fixed during the post-implementation re-verification**
(desktop became free; every deferred visual check was executed):

7. **Phase 05 latent bug — demo isolation**: `reads_demo` matched only
   `demo_name == "reads"`, so `reads-apps` and `reads-power` silently used
   the **live** router: `reads-apps` read the real `$HOME` (no config →
   truthful but wrong-demo rows) and the gsettings rows read the host's
   real dconf. Fixed (`matches!(…, "reads" | "reads-power" |
   "reads-apps")`); the newly captured `p05-apps-detail-alacritty.png`
   shows the fixture-backed rows the Phase 05 report described.
   Reads are harmless, but a demo reading live state violates the
   isolation contract — caught exactly because the capture finally ran.
8. **writes-permission demo was a silent NoOp**: the catalog's
   permission-held row has a `read_only` control, so the staged Edit was
   refused before the policy check and the demo showed nothing. Replaced
   with the established demo-hook pattern
   (`Store::simulate_permission_required`, mirroring `simulate_failed`)
   plus an honest doc comment; the capture shows the lock line + Authorize
   dialog affordance.
9. **writes-light ran on the live environment**: `"writes-light"` was
   missing from the fixture-provider match (it only forced the theme). The
   first capture honestly showed "service not running" live rows — fixed
   by adding it to the writes-demo provider selection; the recapture shows
   the Light theme over the fixture write states.
10. **Capture-script race**: `capture-p05.sh` parsed a `sleep_s` argument
    but never used it, so captures fired ~3.4 s after launch — before the
    demos' 4 s hooks (the reads-apps selection lost the race). Fixed to
    sleep the declared duration; `capture-p06.sh` grew a tall-content
    variant that captures on a temporary headless output (the Phase 01
    D13 technique) because the permission row sits below the fold of the
    physical screen.

## 5. Visual captures and manual verification instructions

**All captures were taken and visually inspected** (the desktop became
free after implementation; each image was verified against its expected
state before committing), in `docs/screenshots/`:

| File | Shows (verified) |
|---|---|
| `p06-writes-dark.png` | Power category on the fixture engine: staged `performance` row ("Unsaved change"), apply bar "2 changes pending" (the second edit is staged against the file-backed Applications row — cross-window sync), Reset-to-default affordances on writable rows, service-provided profile list honestly read-only |
| `p06-writes-conflict.png` | The staged power row in the conflict state: "Changed outside Licoman" + **Reload** / **Compare** (the fake service was genuinely mutated first); no overwrite affordance anywhere |
| `p06-writes-permission.png` | "System updates" in the distinct Permission-required state: lock line "Permission required — org.archlinux.pacman" + **Authorize…** (deny-by-default broker explanation); captured on a temporary headless output (1160×1620) because the row sits below the fold of the physical screen |
| `p06-writes-light.png` | Light theme over the same fixture write states (theme pipeline applied while the write states and apply bar are visible) |
| `p05-reads-dark.png` | (Phase 05 pending capture, retaken) the Source details dialog: backend, source, origin (Managed — dconf system db), freshness+fingerprint, writability, diagnostics, module provenance |
| `p05-reads-light.png` | (Phase 05 pending capture, retaken) fixture gsettings rows in Light theme with origin lines and the honest "cannot be changed yet" notes |
| `p05-apps-detail-alacritty.png` | (Phase 05 pending capture, retaken — and it exposed the demo-isolation bug, §4.7) Alacritty detail with real fixture values ("JetBrains Mono", 11, 0.92), truthful "not present" rows, origin lines |

Reproduce any of them:

```sh
tests/tools/capture-p06.sh all          # writes / writes-conflict / writes-permission / writes-light
tests/tools/capture-p05.sh reads-dark | reads-light | reads-apps
# or run the demos directly (deterministic, no input injection needed):
LICOMAN_DEMO=writes ./target/release/licoman
```

Human-only checks (no input-injection tool exists on this machine):
clicking Apply → observing the visible Applying→Saved transition, the
Undo button appearing, the confirmation dialog's Apply button, and the
reset disclosure flow remain manual; the underlying store transitions are
asserted by the core tests listed above.

## 6. Platform/environment matrix actually tested

| Dimension | Value |
|---|---|
| OS | Arch Linux (kernel 7.2.11-arch1-1), host context |
| Session | Hyprland (Wayland); demos verified headless (`--help`, smoke run) + full test suite headless |
| File writes | Temp fixture roots only (`/tmp/licoman-write-*`, `licoman-writes-demo-*`); committed `tests/fixtures/` byte-identical (git status clean of them) |
| Service writes | In-memory fake only; no real D-Bus service was started, stopped, or written |
| GSettings | Reads unchanged (fixture databases); writes refused everywhere (engine gate tested) |
| Host writes | None: the live run's only writes remain Licoman's own preferences file; demos write under `/tmp` and delete on exit |
| Rendering | Vulkan (mesa/RADV), gpui blade backend; theme pipeline unchanged (Light verified via existing headless palette tests + pending capture) |
| Rust | 1.99.1 stable, edition 2024; **no new dependencies** this phase |
| CI | core job unchanged (no new system packages; all D-Bus/service tests are mocked); workspace job builds the new UI code |

## 7. Security and data-loss assessment

- **Nothing real was written.** The engine attaches only in demo/test
  constructions over temp fixture roots; live providers refuse non-mock
  writes exactly as Phase 05 (the refusal test remains). Committed
  fixtures are byte-identical after the full test suite (git status shows
  no fixture changes). Demo temp environments are swept on next start and
  deleted on clean exit.
- **Conflict safety**: every write re-reads and compares revision +
  expected value first; conflicts abort with the file byte-identical
  (tested). External edits are never overwritten by default; the UI offers
  Reload/Compare only.
- **Recoverability**: bounded private backups (0700/0600, retention 10)
  before every modifying write; readback verification with restore on
  mismatch; failed restores reported as failed with the backup path;
  crash-recovery temp sweeps.
- **Least privilege**: deny-by-default `PrivilegeBroker`; no sudo, no
  password collection, no root execution path exists in the codebase; the
  engine re-checks policy and value coherence independently of the store.
- **Privacy**: logs and the operation log carry ids, backend kinds,
  redacted targets (`~`-prefixed), outcomes and timings only — a
  planted-secret test proves values never leak; backups stay local with
  restrictive modes; no network code exists.
- **Data-loss risk:** none beyond the documented limitations (ACL/xattr
  non-preservation — irrelevant in fixture mode; in-root symlink following
  — bounded by containment, link preserved).

## 8. Remaining work and acceptance gates

| Acceptance criterion (06_SAFE_WRITE_ENGINE.md) | Status |
|---|---|
| No real host setting was modified by implementation or tests | **pass** — fixture/temp-only writes; refusal tests; clean git status for fixtures |
| All file mutations against fixtures are conflict-aware, bounded, structurally preserving and recoverable as documented | **pass** — fingerprint+semantic conflict checks, size/import bounds, toml_edit preservation (byte-exact test), backups+restore+sweeps |
| Deny-by-default privileged broker present; no generic root command execution | **pass** — `DenyAllBroker` is the only broker; no command-execution surface exists; `required_auth` is a denial, not authorization |
| Service mutation path uses typed requests and reports partial/unknown outcomes honestly | **pass** — typed client trait, persistence labels, mismatch=conflict, timeout honesty, no auto-retry, per-row partial success |
| UI never reports Applied before the backend verifies the result | **pass** — single `publish_apply` mapper; "Applied" requires a matching verification; mismatch renders a conflict; store tests assert the flow |
| Failure matrix is covered with machine-checkable tests; notable gaps documented explicitly | **pass** — every §7 row has a named test (§4 table); gaps listed below |
| Security and user-data privacy rules are documented and checked | **pass** — `docs/security/threat-model.md`; redaction asserted by a planted-secret test |
| Release build and test/lint checks pass, or blockers are explained | **pass** — fmt / 236 tests / clippy -D warnings / release build all green |

Known gaps / carried-over gates (all disclosed):

- ~~Four screenshots pending~~ **resolved during re-verification**: all
  four Phase 06 captures and the three pending Phase 05 captures were
  taken and verified (§5). The interactive-dialog walkthroughs remain
  human gates (no input injector exists).
- **ACL/xattr preservation** is documented as a Phase 07 gate (std
  limitation), not implemented.
- **The confirmation/undo/reset dialogs are click-verified only by
  construction** (core transitions are tested; the dialog interactions
  need hands on a free desktop, no input injector exists).
- **Theme switching while a dialog is open** is verified via the unchanged
  `Theme::change` all-windows refresh path and now visually via
  `p06-writes-light.png` (Light palette over the live write states and
  apply bar) plus the headless palette tests.
- **Live GSettings writes, real polkit design implementation, package
  management** — deliberately absent (Phase 07+).
- CI outcome for this phase is unverified until push (no `gh` CLI; check
  the Actions tab after the user pushes and record it in the Phase 07
  report if it failed).

**Stop condition:** this is the end of Phase 06. Per the plan — "Stop
after Phase 06. User-approved real writes start in Phase 07 only after
safety review." The safety review inputs are this report,
`docs/security/threat-model.md` and `docs/architecture/write-engine.md`.

## 9. Reverting this phase

No user configuration is at risk: this phase wrote nothing outside the
workspace and /tmp (screenshots none, fixtures byte-identical, demo temp
trees swept).

To revert the repository to the pre-Phase-06 state:

```sh
cd /home/baris/Work/licoman
git checkout -- . && git clean -fd crates/core/src/write \
  apps/licoman/src/writes_demo.rs docs/architecture/write-engine.md \
  docs/security tests/tools/capture-p06.sh
# keeps Phase 05 (commit 51a373b) exactly as pushed
```

Nothing outside the workspace was modified; no commits or pushes were made.
