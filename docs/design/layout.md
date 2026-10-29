# Layout

Window geometry from the spec (`theme::metrics` is the single source):

| Region | Preferred (logical px) | Minimum |
|---|---|---|
| Main window | 1160 × 760 | 760 × 560 |
| Applications window | 1180 × 760 | 780 × 560 |
| Preferences window | 620 × 420 | 520 × 360 |
| Header | 56 high | search and Preferences always reachable |
| Main sidebar | 244 wide | fixed in Phase 01 (collapsibility deferred, see decisions.md) |
| Applications list pane | 260 wide | shrinks with the window |
| Form content | max 800 wide | avoids ultrawide controls |
| Content padding | 32 | 20 planned when narrow |
| Group separation | 24 vertical | rows separated by hairlines |

Both windows are real native windows sharing the `AppState` entity (theme +
setting state) while keeping independent scope/app selection, search text and
scroll. The Applications nav action opens-or-focuses the second window;
Preferences likewise (single window per surface, no duplicates).

Main window regions (see `docs/screenshots/main-window-dark.png`):

```text
┌──────────────────────────────────────────────────────────────┐
│ TitleBar 56: Licoman · search (Ctrl+K hint) · Preferences ⚙  │
├──────────────┬───────────────────────────────────────────────┤
│ Sidebar 244  │ Scope heading + description + "Mock data" chip│
│  Shared      │ Category group heading + hairline             │
│  Desktop     │ Setting rows (label/desc left, control right) │
│  Distribution│   with state/validation line under the row    │
│  Applications│ …                                             │
│ ──────────── │                                               │
│ Environment  │ ApplyBar (only when changes are pending)      │
└──────────────┴───────────────────────────────────────────────┘
```

Window decorations are the compositor's (CSD shadow/resize via gpui-kit's
`WindowBorder` on Linux); no fake macOS traffic lights are drawn — the
toolkit's `TitleBar` supplies the drag region and (where the platform
provides them) window controls.
