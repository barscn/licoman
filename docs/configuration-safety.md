# Configuration safety — how writes, backups, conflicts and recovery work

This document describes the write path exactly as implemented
(`crates/core/src/write/`, the Phase 06–08 engine). It is the contract the
UI's dialogs disclose and the tests assert.

## What can be written

- **Configuration files** (TOML; mpv's documented grammar), located
  exclusively through the *read* backend's own approved-root logic — a
  manifest cannot name an arbitrary absolute path.
- **GSettings keys**, through the native gio API, only inside a detected
  GNOME session (fail-closed desktop gate).
- **Power-profiles properties**, typed over the system bus.
- **Nothing else.** Xfconf is read-only in this build; privileged
  operations (packages, services, firewall) do not exist; no command
  execution surface exists anywhere in the write path.

## The pipeline for one write

1. **Policy derived by core** — the binding family and the row's risk
   decide the required authorization and confirmation; a module can only
   make this stricter, never weaker.
2. **Value gate before I/O** — the typed value is validated against the
   descriptor (type, range, enum set) before anything touches a file or
   service.
3. **Target resolution and shape checks** — the file must be inside the
   approved roots, a regular file, owned by the current user, and not a
   symlink at the target path; escapes (including via symlinks) are
   rejected before any content is read. Extended attributes (including
   POSIX ACLs) are read first and preserved onto the replacement — any
   failure refuses the write up front.
4. **Conflict checks** — the source's current fingerprint and semantic
   value must match what the UI last saw. If another program changed the
   configuration in the meantime, Licoman refuses and offers
   Reload/Compare; it never overwrites an external change.
5. **Minimum patch** — exactly the key's own value is edited (a
   formatting-preserving TOML edit; a single raw line for mpv configs).
   Everything else in the file is preserved byte-for-byte.
6. **Backup** — the affected file is copied to a private backup root
   (`$XDG_DATA_HOME/licoman/backups/…`, mode 0700/0600), bounded to 10
   generations per target.
7. **Atomic staging** — the patched content is written to a securely
   created temp file (`O_EXCL`, 0600) on the same filesystem, validated,
   the original mode re-applied, fsynced, then atomically renamed; the
   parent directory is fsynced. A crash between staging and rename can
   never leave debris (stale temp files are swept on the next run).
8. **Readback verification** — the new value is read back through the same
   approved read path and compared. On mismatch the backup is restored
   automatically, and Licoman reports honestly whether the restore
   succeeded.
9. **Outcome publication** — the row shows exactly what happened
   (verified apply, conflict, permission required, failure) with the next
   action available.

## Recovering from a failed or partially applied change

- **Restore from a backup:** backups live under
  `$XDG_DATA_HOME/licoman/backups/<sanitized-setting-id>/<timestamp>-<file>`
  as byte-exact copies. Copy one back over the configuration file (or let
  the automatic restore do it — it runs before the error is reported).
- **Staged edits survive failures:** a refused write keeps your edit
  staged; nothing is half-applied. Undo reverses a *verified* apply while
  the source still holds the applied value.
- **GSettings:** a reset uses the platform's own reset semantics
  (`g_settings_reset` — your override is removed, the schema default
  returns); a failed write is a failed call, nothing lands partially.
- **Power profiles:** a failed property set leaves the daemon's current
  profile untouched (the readback verifies it).

## What is never done

- No install/uninstall of packages, no service starts or stops, no sudo,
  no password collection, no root paths.
- No automatic writes on startup, discovery, test runs or theme switches.
- No telemetry or remote calls of any kind — safety and privacy live on
  the same principle: Licoman acts locally, explicitly, and reversibly.
