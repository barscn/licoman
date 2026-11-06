# Layout

Window geometry from the spec (`theme::metrics` is the single source):

| Region | Preferred (logical px) | Minimum |
|---|---|---|
| Main window | 1160 × 760 | 760 × 560 |
| Header | 56 high | search and Settings always reachable |
| Main sidebar | 244 wide | fixed in Phase 01 (collapsibility deferred, see decisions.md) |
| Applications list pane | 260 wide | shrinks with the window |
| Form content | max 800 wide | avoids ultrawide controls |
| Content padding | 32 | 20 planned when narrow |
| Group separation | 24 vertical | rows separated by hairlines |

Licoman is a single-window app (decision D61): the Applications and
Settings panels render in the content area, and every scope change slides
the incoming pane in from the direction of the newly selected sidebar item
(±24 px, 200 ms ease-out-cubic + fade). There is one shared `AppState`
entity and one window.

Main window regions (see `docs/screenshots/main-window-dark.png`):

```text
┌──────────────────────────────────────────────────────────────┐
│ TitleBar 56: Licoman · search (Ctrl+K hint) · Settings ⚙      │
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
