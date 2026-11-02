# Phase 07 integrations — supported combinations and limits

The three Phase 07 vertical slices plus the optional diagnostics module,
with the exact environments they were verified on and the honest limits.
This page is deliberately precise: nothing here claims "all Linux
supported".

## Integration A — Alacritty (TOML file backend)

| Dimension | Supported | Not supported (honest state shown) |
|---|---|---|
| Alacritty version | ≥ 0.13.0 (TOML configuration era), detected via bounded `alacritty --version` | < 0.13 (legacy YAML era): distinct "legacy YAML" unavailable state; version undetectable: writes disabled, reads stay available |
| Configuration | `alacritty.toml` on the documented search paths, `general.import` merges (last loader wins) | legacy `alacritty.yml`-only setups (recorded, never edited); imports winning a key (engine refuses to edit an imported file) |
| Options | the manifest's file-bound options, each validated against the detected version (D50); enum values restricted to the era-stable set | `shell`/`working_directory` (moved to `terminal.shell` in 0.14 — mock-bound with that reason); config creation when absent (deferred: requires explicit user intent UI, absent app on the dev machine) |
| Verification | Arch Linux, Alacritty **not installed** → the honest absent state; full write pipeline machine-tested on fixture roots (258-test suite); L1-style live file writes not executed on this machine (no target) | — |

Live-write prerequisites on a given machine: Alacritty installed, a TOML
config on the documented paths (or explicit user intent to create one),
version detectable. The engine additionally refuses: imported winners,
symlink escapes, foreign-owned files, files with unrestorable xattrs/ACLs
(D53), fingerprints/values changed since the read.

## Integration B — GNOME / GSettings (native backend)

| Dimension | Supported | Not supported |
|---|---|---|
| Session | positively identified GNOME (`DesktopIdentity::Gnome`) | any other desktop or unknown: module inapplicable **and** the engine's desktop gate fails closed (D51) — a schema being installed never makes a control writable |
| Schemas | keys of `org.gnome.desktop.interface` declared by the module, present in the schema database | missing schema/key: distinct unavailable states, no write attempt |
| Writes | native libgio `set_value` (schema-typed, range-checked), `reset` (real reset semantics), readback after `g_settings_sync()` | shell tools, dconf internals — structurally impossible (no such field/code) |
| External changes | gio `changed` signals (dedicated watcher thread) + the dconf-db file watcher → debounced refresh; pending edits survive and conflict at apply time | — |
| Verification | **live, user-approved (L1)**: set `clock-show-weekday` → true, read back by Licoman **and** `gsettings get`, reset, both verified; real dconf db fingerprint byte-identical before/after (disposable-root run, private session bus) | — |

## Integration C — PowerProfiles (D-Bus)

| Dimension | Supported | Not supported |
|---|---|---|
| Service | `org.freedesktop.UPower.PowerProfiles` or legacy `net.hadess.PowerProfiles`, passively probed (never activated) | absent service: truthful "not running" rows for profile, available profiles and performance state; no fake dropdown |
| Reads | `ActiveProfile`, dynamic `Profiles` (both the legacy `as` and current `aa{sv}` shapes), `PerformanceDegraded`, holds count | — |
| Writes | typed `Properties.Set` on `ActiveProfile`, readback-verified, persistence reported `Unknown` (the interface documents no guarantee) | automatic retry of a timed-out mutation; starting/installing services |
| Verification | write path machine-tested against the typed fake (faults: denial, timeout, change-after-apply); live service absent on the dev machine → honest states (screenshot `p07-live-power.png`) | — |

## Optional — Hyprland read-only diagnostics

Read-only runtime facts (compositor version, active workspace, monitors)
from the documented IPC socket, each labeled "Runtime only". Verified live
on Hyprland 0.56.2 and 0.57.0 (screenshot `p07-live-hyprland.png`).
Requests are single bare commands without a trailing newline (0.56 rejects
newline-terminated query requests with "unknown request"). No Hyprland
configuration is ever written in this phase.

## Environment notes recorded during verification

- dconf isolation: a process-level `XDG_CONFIG_HOME` override does **not**
  isolate writes (the session bus's dconf-service owns the database path);
  disposable runs need a private session bus with a self-started
  `dconf-service` — see the live checklist and the Phase 07 report.
- Hyprland 0.57 changed `hyprctl dispatch`/`keyword` to a Lua shim; the
  screenshot harness no longer drives float/resize dispatches and crops to
  the window's actual geometry instead.
