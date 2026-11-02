# Phase 07 — Controlled Live Verification Checklist

**Status:** plan + harness. Nothing in this document runs automatically; the
Ignored test harness (§3) executes **only** after the exact operation list
(§4) has been approved, one operation at a time.

**Rule (07_FIRST_REAL_INTEGRATIONS.md):** live writes require explicit
**per-test user approval** and are never run as part of `cargo test`, CI,
application startup, discovery or without an explicit Apply. Default run
mode is read-only until the user deliberately invokes an Apply for a
supported control. Approval of this task file is **not** authorization to
edit arbitrary real settings.

## 1. Machine facts this checklist was written against (2026-11)

| Fact | Value | Consequence |
|---|---|---|
| Desktop / session | Hyprland 0.56.2, Wayland | GNOME module is **inapplicable** (compat: `Requires "GNOME"`); its rows are hidden and the engine's desktop gate is closed — exactly the "no false desktop control on a mismatched desktop" requirement |
| GSettings schemas | real (`/usr/share/glib-2.0/schemas/gschemas.compiled`), dconf user db present | The native write stack (libgio → libdconf) is fully exercisable |
| Alacritty | **not installed**, no config file on any documented search path | Integration A shows the honest absent state; no live file-write target exists unless Alacritty is installed |
| power-profiles-daemon | **not running** (only `org.freedesktop.UPower` on the system bus) | Integration C shows the honest absent state; the write path is verified against the typed fake service |

## 2. What is verified without touching any real setting

- Fixture test suites (247 core tests at phase end): the whole Phase 06
  write matrix plus the Phase 07 additions — Alacritty version gates
  (unknown/legacy versions refuse writes, reads stay available), xattr/ACL
  preserve-or-refuse, GSettings write round trip through the **native API
  on a per-process memory backend + committed fixture schemas**, desktop
  gate fail-closed/open, PowerProfiles typed fake faults, Hyprland IPC
  parser.
- Live binary smoke runs (no writes): discovery, real reads (gsettings
  origin `User`/`Default`), GSettings gate `open=false` on Hyprland,
  Hyprland diagnostics rows reading the real compositor IPC
  (version `0.56.2`, active workspace, monitors), **zero write events**.

## 3. The Ignored live-test harness

`crates/core/tests/live_p07.rs` is a Rust integration test that is
`#[ignore]`-by-default **and** additionally requires
`LICOMAN_LIVE_P07=1` in the environment. It never runs in `cargo test`,
CI, or by accident. It drives the **same store → engine → backend path the
UI drives** (dispatch Edit → ApplyAll → apply_complete), which is how
verification is automated on a machine without an input injector; the UI's
interactive dialogs were verified by construction in Phase 06 and remain a
human gate.

Two modes, selected explicitly per run:

- **disposable (default):** the run executes under a **private session bus**
  (`dbus-run-session`) with `XDG_CONFIG_HOME` at a fresh temp directory,
  and the harness **pre-spawns `dconf-service` with that environment** so
  the private session's dconf writes land in a **throwaway user database**.
  Real native stack end to end, zero contact with the real dconf db. The
  temp tree is deleted afterwards.
- **real (only with separate, explicit approval, §4 op L2):** targets the
  real user database. Requires the pre/post fingerprint recording below.

**Isolation lessons (recorded 2026-11, cost one incident — see the Phase 07
report):**

1. A process-level `XDG_CONFIG_HOME` override isolates dconf **reads**
   only: libdconf sends writes to the session bus's `dconf-service`, which
   resolves its database from **its own** environment. The first L1 attempt
   (no private bus) therefore wrote the real database; it was restored
   immediately with a native `gsettings reset`, verified by fingerprint.
2. A private session bus alone is not enough either: dbus activation may
   route `ca.desrt.dconf` through the systemd user instance, whose
   environment carries the real config home. The harness pre-spawns
   `dconf-service` itself with the temp environment so the private session
   has exactly one service, on the disposable database.
3. `g_settings_set_value`/`g_settings_reset` queue at the dconf client
   level; without `g_settings_sync()` the engine's readback would verify
   its own client-local view. The write engine syncs before verifying, so
   readbacks are service-authoritative (this is now part of the backend,
   not just the harness).

## 4. Proposed operations (each requires explicit approval)

| # | Operation | Target | Effect | Verification | Rollback |
|---|---|---|---|---|---|
| L1 | Set `org.gnome.desktop.interface clock-show-weekday` → `true`, read back, then **reset** | **Disposable** dconf db (temp `XDG_CONFIG_HOME`) | A throwaway db changes; nothing real is touched | harness readback + `dconf dump` of the temp db + `gsettings get` **with the temp config home** | harness reset (removes the override); temp tree deleted |
| L2 | Same as L1 against the **real** user db | `$XDG_CONFIG_HOME/dconf/user` (currently `false`) | The top-bar weekday display of a *future GNOME session*; no running GNOME process reads it on this machine | harness readback + `gsettings get org.gnome.desktop.interface clock-show-weekday` | harness **reset** (GSettings reset semantics — removes the override); verified back to `false` |
| L3 | Alacritty live write (e.g. `font.size`) | real `~/.config/alacritty/alacritty.toml` | None unless Alacritty is installed first (user action: `pacman -S alacritty`); then: version detected ≥ 0.13, minimal config created **with explicit intent**, engine writes with backup + verification | Licoman readback + `alacritty --version` + opening Alacritty | Licoman **Undo** (inverse write) or the engine's bounded backup |
| L4 | PowerProfiles live write (`ActiveProfile`) | real system-bus service | None unless the user installs/starts power-profiles-daemon (user action; Licoman never starts services) | Licoman readback + `busctl get-property` | Licoman **Undo** |

**Not proposed:** anything privileged, any package operation by Licoman,
any Hyprland config write, any dconf internal editing, bulk operations.

## 5. Per-operation record template

Each executed operation is recorded in `PHASE_07_REPORT.md` §"Live
verification" with: exact command/harness run, before fingerprint, outcome,
native-tool verification output, rollback result, and any imperfect
recovery — no hidden state.

## 6. Abort conditions

Any unexpected refusal, any readback mismatch, any backup/restore anomaly,
or a verification tool disagreeing with Licoman → stop the sequence and
report; never retry a failed non-idempotent write automatically.
