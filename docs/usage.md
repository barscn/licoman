# Using Licoman

## Scopes

The main window's sidebar groups everything Licoman knows about into four
scopes:

- **Shared** — system-wide capabilities (appearance, power, audio,
  networking, portals) provided by services on this machine.
- **Desktop** — desktop-environment settings (GNOME/GSettings, Xfce/Xfconf,
  Hyprland compositor diagnostics), shown for the environment that is
  actually detected.
- **Distribution** — distribution-level information. Privileged operations
  (package management, services, firewall) are disabled by design.
- **Applications** — opens the dedicated Applications window.

## The Applications window

The Applications window is a separate native window: the installed
application list (with filter) on the left, the selected application's
settings on the right. Applications Licoman does not support yet stay
visible with a real integration-status explanation — they are never
hidden just because no module exists.

## Reading values

Every row carries provenance. The status line under a setting states where
the current value came from — *Set by you*, *Default*, *Live service
state*, *Managed by your system* — and **Source details…** opens the
technical provenance (backend, exact target, origin notes, diagnostics;
metadata only, never contents). A value the system cannot currently serve
is shown as unavailable with the reason, never replaced by a guess.

## Changing values

- Edits are staged until you press **Apply** (rows whose policy is
  `immediate` apply at edit time and say so).
- High-risk or restart-relevant changes ask for confirmation first, and
  the dialog names the effect and the target.
- After an apply, the row shows the verified outcome; **Undo** (while
  available) and **Reset to default…** (with a disclosure of exactly what
  a reset does) are always visible actions, never hidden menus.
- Licoman never writes without your explicit action, and the write path
  runs conflict checks and keeps a bounded backup — see
  [configuration-safety.md](configuration-safety.md).

## Search

`Ctrl+K` (or the header search) opens the global palette across **all**
scopes and applications — 1,000+ settings are searched from a
pre-computed index in well under a millisecond. Palette hits in the
Applications scope navigate into the Applications window.

## Theme

Preferences → Appearance: **Dark** (first-run default), **Light** and
**System** (follows the desktop preference via the settings portal).
Theme changes apply to every open Licoman window immediately, without a
restart and without losing state.

## Keyboard

The interface is keyboard-reachable (tab navigation through rows,
controls and dialogs; the palette is fully keyboard-driven). Some
multi-window flows still need verification with assistive tools on your
distribution — see [design/accessibility.md](design/accessibility.md).

## Privacy

Licoman reads only what it needs to display the settings you open, writes
only its own preferences file (plus the explicit, backed-up configuration
writes you trigger), and sends nothing anywhere. Logs contain identifiers
and outcomes — never setting values. Details in [privacy.md](privacy.md).
