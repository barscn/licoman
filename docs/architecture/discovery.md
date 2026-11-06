# Environment Discovery Architecture

**Status:** as of Phase 03 (2026-10-31). Implementation:
`crates/core/src/discovery/` (models, parsers, probes) +
`crates/ui/src/app_state.rs` / `environment_watch.rs` (threading, caching,
watching).

Discovery is **read-only, explainable capability detection**. It determines
which modules and controls may appear; it never edits user, desktop,
package-manager or system configuration, never executes a `.desktop` `Exec`
line, and never launches a daemon to enable a module.

## What the snapshot holds

`licoman_core::discovery::EnvironmentSnapshot`:

| Field | Source module | Contents |
|---|---|---|
| `distro` | `os_release` | `ID`, `ID_LIKE`, `NAME`, `VERSION_ID`, `PRETTY_NAME` + `OsReleaseStatus` + evidence |
| `kernel` | `os_release` | `/proc/sys/kernel/osrelease` + `std::env::consts::ARCH` (informational only — never distro identity) |
| `execution_context` | `os_release` | host / container (docker, podman, kubepods, lxc) / WSL, from filesystem + cgroup markers |
| `session` | `session` | desktop identity + candidates, compositor pairing, display protocol, ambiguity note |
| `installed_apps` | `apps` | deduplicated XDG applications with origin, restriction and Flatpak facts |
| `available_backends` | `backends` | package managers, D-Bus session bus, GSettings schema database, anticipated app-config locations |
| `problems` | all | every isolated detection failure, with kind + user message + technical detail |

Every fact carries `Evidence { source, detail, confidence }`. Confidence is
High/Medium/Low and is *displayed*, not hidden — environment variables are
hints, not authority.

## Sources, precedence and confidence

### `/etc/os-release` (High)

Parsed per the freedesktop standard: `KEY=value`, optional single/double
quotes, comments on their own lines. Missing/malformed/future keys are
tolerated (unknown keys are kept, malformed lines are counted into a
`os-release-malformed` problem). Matching (families like `arch`, `rhel`,
`debian`) uses **IDs only**, never translated pretty names. A missing file
yields `OsReleaseStatus::Missing` and an unknown distribution — the app
stays fully usable.

### Desktop/session variables (High → Low)

`XDG_CURRENT_DESKTOP` (High; colon-delimited, each element matched
separately — never a false single-string match, first element wins per the
spec's preference order), `XDG_SESSION_DESKTOP` (Medium),
`DESKTOP_SESSION` (Low; generic values like `default` are ignored),
`XDG_SESSION_TYPE` (High for the protocol), `WAYLAND_DISPLAY`/`WAYLAND_SOCKET`
(Medium) and `DISPLAY` (Medium). Known desktops: GNOME, KDE Plasma, Xfce,
COSMIC, Hyprland, Sway — anything else is honestly `Unknown`; detection is
**not** claimed to be complete.

Three separate facts: desktop identity, compositor/WM (a *knowledge table*
over the detected desktop — e.g. GNOME→Mutter — not a probe), and display
protocol. GNOME-on-Wayland is therefore a different fact from GNOME. When
indicators disagree, all candidates stay visible with their confidences and
an `ambiguity` note is recorded — never one arbitrary winner. Both Wayland
and X11 sockets visible → nested/bridged-session note (XWayland), no crash,
no destructive assumption.

### Installed applications (High, per entry)

XDG application directories are scanned **in basedir precedence order**
(user `$XDG_DATA_HOME` first, then `$XDG_DATA_DIRS`, plus the Flatpak
export directories `flatpak/exports/share/applications` in the user and
system data roots). The dedup key is the **desktop-file id** (the file name,
e.g. `org.gnome.Nautilus.desktop`), not the display name; the first
directory in search order wins and shadowed duplicates are counted.

Per-entry semantics (all read-only, `stat`-only):

- `Type=Application` only (Links/Directories skipped, silently).
- `Hidden=true` → ignored entirely (spec: equivalent to deletion).
- `NoDisplay=true` → kept, flagged; visible in **All installed** only.
- `TryExec` → resolved against the PATH snapshot with a `stat`; missing →
  entry ignored (per spec) with an explanatory problem entry.
- `OnlyShowIn`/`NotShowIn` → evaluated against the current desktop's raw
  `XDG_CURRENT_DESKTOP` elements (case-insensitive); restricted entries stay
  listed but are excluded from the primary view.
- `X-Flatpak` → recorded as the Flatpak application id; origin is
  `flatpak-user`/`flatpak-system` by directory.

Hard bounds: 1 MiB per file, only regular files (a planted FIFO or device
node is skipped, never read), invalid UTF-8 and spec-invalid entries become
`desktop-entry-invalid`/`desktop-entry-skipped` problems — a broken entry
can never prevent the scan of the others or the launch of Licoman.
`Exec` lines are **never executed and never stored**; `Icon` values are kept
as unvalidated display metadata, never used to locate anything.

Privacy: only the current user's XDG directories and system data dirs are
traversed; no other user's home directory is ever read, and no log line ever
carries file contents or setting values (ids and counts only).

### Backend/service probes (passive, bounded)

- **Package managers** (`package-manager:<name>`): PATH `stat` for
  pacman/dnf/apt/zypper/apk/emerge. Informational — presence of a binary is
  explicitly **not** permission to change packages.
- **D-Bus session bus** (`dbus-session`): a hand-rolled, dependency-free
  client performs the SASL EXTERNAL handshake plus `Hello` and `ListNames`
  with 2-second timeouts, then disconnects. No service is activated and no
  daemon is launched; every failure mode (no address, connect refused, auth
  rejected, timeout, malformed reply) degrades to "unavailable" plus a
  `dbus-probe-failed` problem — the documented safe state.
- **GSettings schemas** (`gsettings-schemas`): the compiled
  `glib-2.0/schemas/gschemas.compiled` GVDB databases are parsed directly
  (minimal reader over `gvdb-format.h`; verified against a real GLib file).
  Schema ids **and key names** are enumerated; databases merge by schema id
  with first-directory precedence. Running the `gsettings` binary is
  deliberately avoided: it can auto-activate `dconf-service`, which the plan
  forbids ("avoid launching daemons solely to enable a module"). Only
  little-endian files are supported (both supported targets are LE).
- **Anticipated config locations** (`config:<app>`): `stat` of
  `$XDG_CONFIG_HOME/<dir>` for the apps with registered modules. This is
  what separates *backend available* from *module available* in the UI.

## The five conditions stay separate

A detected app is not automatically supported:

| Condition | Phase 03 source |
|---|---|
| **Installed** | desktop entry discovered |
| **Module available** | registered module matches (desktop-file id, High; `StartupWMClass`, Medium) |
| **App version compatible** | **not claimed** — arrives with real backends (Phase 05+) |
| **Backend available** | `config:<app>` probe available |
| **Writable** | **not claimed** — Phase 06+ |

The merge (`matching::build_app_list`) is deterministic and sorted; module
matches are exact, case-insensitive, suffix-stripped (`alacritty.desktop` ⇔
`alacritty`) — no fuzzy heuristics. Unknown apps render
"No Licoman integration available yet." with their facts, never fake
settings.

## Threading, refresh and cache

- Core discovery is a plain synchronous function (`discover`) over injected
  `DiscoverySources` — no threads, no timers, no I/O surprises. A staged
  variant (`DiscoveryRun::run_next_stage`) drives the same four stages
  (distribution → session → applications → backends) one at a time.
- The UI adapter (`licoman_ui::app_state::EnvironmentState`) runs stages on
  GPUI's background executor, publishing a partial snapshot after every
  stage. The UI thread never blocks; loading and partial results are
  visible (Applications panel).
- Cancellation: `ScanControl` (an `AtomicBool`) is checked between stages
  and between file batches; a user-visible **Refresh** cancels the in-flight
  scan and starts a new one. A generation counter ensures stale scans never
  publish over a newer one.
- The cache holds **safe metadata only** — the snapshot and the derived
  app/module list. It is invalidated by explicit Refresh and by the
  debounced (500 ms) inotify watcher over the application directories
  (`environment_watch.rs`, `notify` crate, event-driven: zero wakeups when
  nothing changes). It **never** caches effective setting values; those
  invalidation contracts arrive with the Phase 05 read backends.
- Failure isolation: a bad desktop entry (or an unreadable directory, or a
  failed probe) becomes a `DetectionProblem`; scanning continues; Licoman
  launches.

## Diagnostics export

`licoman --diagnostics` runs the same discovery headless and prints the
redacted export (`discovery::export::diagnostics_text`): distribution,
session + evidence, package managers, application counts by origin, backend
statuses, and all problems. Rules: paths under `$HOME` are redacted to `~`;
`Exec` lines, icon payloads, file contents and setting values are never
included. The export contains only non-secret discovery facts.

## Known limitations

- Desktop detection covers the plan's seven cases; anything else is
  `Unknown` with the evidence shown.
- `XDG_SESSION_TYPE` values other than wayland/x11/tty fall back to socket
  variables and are flagged as unrecognized.
- GVDB parsing is little-endian only; big-endian systems fail header
  validation and report the schema database as unreadable (with a problem
  entry), which is fail-closed.
- Flatpak system exports are read from `/var/lib/flatpak/exports`; runtimes
  and apps without exported `.desktop` files are invisible to any
  `.desktop`-based scanner — the plan explicitly accepts this ("do not
  assume all installed software has a .desktop file").
- Container environments may report host identity that differs from the
  services actually reachable; the execution context is surfaced as
  evidence, never used to gate a capability by itself.
