# Security Threat Model — Licoman

**Scope:** Phase 06 (safe write engine, fixture-only). Built on the Phase 05
read layer. Live writes are gated to Phase 07; this document is the review
baseline that gate refers to.

## Assets and trust boundaries

| Asset | Where it lives | Who may touch it |
|---|---|---|
| User configuration files (`$XDG_CONFIG_HOME`, dotfiles, `/etc/*` configs) | outside Licoman | the user and their applications; Licoman only via the write engine |
| dconf/GSettings state | platform databases | the platform; Licoman reads only (writes gated to Phase 07) |
| D-Bus service state | live services | the services; Licoman via typed property calls only |
| Licoman preferences | `$XDG_CONFIG_HOME/licoman/preferences.toml` | Licoman (the one file it may always write) |
| Write-engine backups | `<data-root>/backups/<setting>/…` (`0700`/`0600`) | Licoman, for restore |
| Module manifests | built-in (embedded) or user-provided files | data only — can never execute |

Process boundary: Licoman runs as the **normal user**, unprivileged, one
process. There is no root execution path, no embedded `sudo`, and no password
collection. Anything requiring elevation is refused by the privilege broker
(deny-by-default in this build).

## Threats and mitigations (Phase 06 state)

### T1 — A malicious or buggy module destroys user configuration

A manifest is untrusted data. Mitigations, in order:

- Manifests are validated data-only definitions (Phase 04): no shell text, no
  commands, no absolute paths, no `..` — the grammar rejects them, and there
  is no field that could carry an executable payload.
- The write target is resolved by **backend logic** (approved roots, search
  paths, import merge), never from manifest-provided absolute paths.
  Containment is enforced on canonicalized paths; symlink escapes and
  out-of-root imports are refused, never followed.
- Writes modify the **minimum key(s)** through `toml_edit`; comments,
  whitespace, ordering and unrelated sections are preserved and asserted by
  tests.
- Every write is preceded by a revision check (fingerprint) **and** a
  semantic check (expected value); a mismatch aborts with a conflict instead
  of overwriting external changes.
- Every modifying write keeps a bounded, private backup of the affected file
  only; verification failure triggers restore, and a failed restore is
  reported as failed (never a fake success).

Residual risk: in-root symlinks are followed on write (the canonical target
inside the approved roots is updated; the user's link is preserved). This
supports managed-dotfile layouts but means Licoman may modify a file
*reachable through* an approved-root link. Escapes remain impossible.
Documented as decision D47; revisit before live writes.

### T2 — A module escalates its own privileges

`required_auth` (or any permission scope) routes the request to the
`PrivilegeBroker`, whose only Phase 06 implementation refuses everything.
Modules cannot construct commands (no such field exists), mark themselves
safe (policy derivation is core-owned and conservative), or reach the engine
except through the provider's typed intent path. `[requires_root]`-style
declarations are **not authorization** — they are denials.

Future (Phase 07+, documented design): a minimal polkit-checked helper with
an allowlist of operations, validation by the privileged side, caller
identity checks, and bounded input data types over D-Bus or similar. Not a
general command runner. This build contains none of that code.

### T3 — Sensitive values leak through logs, diagnostics or the UI

- Structured logs carry ids, backend kinds, counts, elapsed times and
  **redacted targets** (home/config-home prefixes collapse to `~`) — never
  values, file contents or credentials. The operation log is bounded
  (100 entries) and redacted by construction; a test plants a secret-shaped
  value and asserts it never appears in any record.
- Source details and dialogs show metadata (paths, origins, fingerprints),
  never configuration contents — inherited from the Phase 05 envelope.
- The diagnostics export remains the redacted Phase 03 export; no new
  surfaces write to it.
- Backups copy only the affected target into `0600` files under `0700`
  directories. They never travel anywhere (no network code exists in the
  codebase).

Residual risk: backups of a secret-bearing config are secret-bearing files
on the same disk. Acceptable for a local restore mechanism; retention is
bounded (10 per target); documented.

### T4 — Race conditions corrupt configuration

- External edits between read and write are caught by the fingerprint
  (recomputed through the same read path) and the semantic before-value
  check; the abort leaves the file byte-identical (tested).
- Staging happens in a securely created temp file (`O_EXCL`, `0600`) on the
  same filesystem, then an atomic `rename`, then a parent-directory `fsync`.
  A crash before the rename leaves the original intact and a swept-away
  temp file (constructor + per-write sweep, tested).
- The store's write pass is generation-guarded; repeated clicks and
  overlapping commands cannot double-apply (the store refuses re-entry while
  a pass is in flight).

### T5 — Denial of service via hostile inputs

Bounded everything: 256 KiB per config file, ≤ 8 import depth, ≤ 32 files,
bounded backup retention, bounded operation log, bounded timeouts on every
service call (injected clients included), bounded undo history. Invalid
values are rejected **before any I/O** (engine value gate).

### T6 — A compromised or hung service gives false results

Service mutations verify by readback; a service that reports success but
holds a different value produces an honest conflict (the service is
authoritative), never an "Applied" badge. A hung service times out with the
message that it *may* have applied the change — Licoman never auto-retries
a non-idempotent mutation. Persistence is reported as `persistent`,
`runtime_only` or `unknown`; `unknown` is never presented as durable.

## Explicitly out of scope (this phase)

- Live GSettings/dconf writes (Phase 07 gate; the engine refuses them).
- Package-manager or distribution administration changes (later security
  milestone).
- Any real privileged helper, polkit integration or authentication UI.
- Multi-file atomic transactions (impossible in general; the engine
  documents partial success and per-row compensation instead).

## Review checklist used for this phase

1. No real host file, dconf database or service was touched by
   implementation or tests (fixture roots + temp dirs only).
2. Every file mutation is conflict-aware, bounded, structurally preserving
   and recoverable (matrix §7 of the phase document — machine-checked).
3. Deny-by-default privilege broker present; no root command execution path
   exists in the codebase.
4. Service path uses typed requests; partial/unknown outcomes are honest.
5. The UI never reports "Applied" before a verification matched the request.
6. Logs and operation records are redacted by construction (tested).

## Phase 07 state — live writes

The Phase 06 boundaries above are updated, not weakened:

- Live providers carry a real write engine (D49); every write still passes
  the store's policy gate, the engine's independent policy/value gates, and
  requires an explicit user Apply/Confirm. Nothing writes at startup, in
  discovery, in tests, or in CI.
- The GSettings path uses only the native libgio API behind a fail-closed
  desktop gate (D51); no shell tools, no dconf internals.
- The Alacritty path is version-gated fail-closed (D50) and preserves
  xattrs/ACLs or refuses the write (D53).
- The PowerProfiles path is a typed property set with readback (D52).
- The Phase 06 residual risks were closed or re-recorded: ACL/xattr
  preservation is now preserve-or-refuse; in-root symlink following (D47)
  remains, bounded by containment.

### Incident record (L1 first attempt, 2026-11)

The first disposable-isolation attempt used a process-level
`XDG_CONFIG_HOME` override without a private session bus; dconf writes are
routed by the **service's** environment, so one test key
(`org.gnome.desktop.interface clock-show-weekday` → `true`) reached the
real user database. Recovery was immediate and verified: native
`gsettings reset`, `gsettings get` back to `false`, the key absent from
`dconf dump`, no other key touched. The isolation procedure was fixed
(private session bus + self-started `dconf-service` with the disposable
environment) and the approved operation was then executed cleanly with the
real database fingerprint byte-identical before/after. The full timeline is
part of the project's recorded verification history (summarized in
[CHANGELOG.md](../../CHANGELOG.md)).

---

## Phase 08 addendum — Xfconf and mpv (2026-11-04)

The Phase 08 expansion was reviewed against this threat model when the two
new backend families landed; conclusions:

- **New assets:** none beyond Phase 07. Xfconf rows are read-only; mpv
  writes reuse the audited file pipeline (fingerprint conflicts, 0600
  backups, atomic staging, readback) with a grammar-aware one-line patcher
  validated against malformed input, external edits and profile-scoped
  traps in fixtures.
- **Passive probing extended:** xfconfd is D-Bus-activatable; the Xfconf
  backend gates every call behind a passive ListNames check. Verified
  end to end: a full application run with the daemon stopped leaves it
  stopped (no activation, ever).
- **The manifest grammar grew safely:** the `xfconf` binding names a channel
  identifier and a `/`-rooted property path (shell-metacharacter and `..`
  rejection shared with the file-key rules); `WriteTarget::ReadOnly` is a
  typed target the engine refuses unconditionally — a manifest cannot
  reach any new write capability.
- **Dynamic module loading remains unimplemented** (see
  `module-trust.md` for the review it requires); no runtime code of any
  kind, and no GPUI JavaScript runtime, is enabled.
- **Logs stay value-free** (the planted-secret test still passes); Xfconf
  source details carry channel/property names and counts only.
