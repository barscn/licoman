# Accessibility

## Keyboard paths (implemented bindings)

| Keys | Context | Action |
|---|---|---|
| `Ctrl+K` | MainWindow / AppsPanel | Open search palette (main) — bound per context; the apps panel focuses its filter |
| `Ctrl+F` | MainWindow / AppsPanel | Focus current search field |
| `↑` / `↓` | SearchPalette | Move palette selection |
| `Enter` | palette input | Accept highlighted result |
| `Esc` | SearchPalette | Dismiss palette, focus returns to the header search field |
| `Ctrl+,` | global | Open or focus Licoman Preferences |
| `Ctrl+Q` | global | Quit |
| `Tab` / `Shift+Tab` | Root (toolkit) | gpui-kit Root layer provides tab traversal |

Every scope, app list row, palette result and action button is a focusable,
clickable element; controls carry accessibility labels (`Switch`,
`Select`, `Input`, `Button` set AccessKit names through gpui-kit's
interaction layer; sidebar items expose `accessibility_label`).

## Focus behavior

- Opening the palette focuses its input; closing returns focus to the header
  search field (the invoking control).
- No focus traps: overlays are plain elements in the window's focus order;
  dialogs from the toolkit's Root layer handle restore.
- Theme changes refresh every window through `Theme::change`; focus is
  preserved because no view is rebuilt from scratch.

## Manual keyboard-only verification checklist (human session required)

Still **not executed** automatically. An automated attempt was made on
2026-10-30 and is documented in decisions.md D12: no injection tool exists on
the machine; Hyprland's `sendshortcut` synthesizes keys that never reach the
gpui client; a WIP `zwp_virtual_keyboard_v1` injector
(`tests/tools/vk_injector.c`) does not work against this Hyprland build yet.
The following must be walked through by a person on a real session:

1. Launch → Tab to sidebar → select Desktop scope → open the Applications view.
2. `Ctrl+K` → type "power" → `↓`/`↑` → `Enter` → palette closes, scope jumps.
3. `Ctrl+,` from each window → Preferences opens/focuses → switch theme with
   keyboard only → both windows repaint without focus loss.
4. `Esc` dismisses the palette; focus lands back in the header search field.

## Contrast plan

Dark body text on canvas and light body text on canvas clear WCAG AA for
normal text by inspection; a **measured** pass over all combinations
(disabled controls, borders vs. adjacent fills, accent-filled button labels,
tooltips, high-contrast modes) is scheduled with Phase 09 together with the
accessible-names audit. Non-text boundaries (hairlines, focus ring) target
3:1; the focus ring uses the `accent` token which sits well above it against
both canvases in rendered screenshots.

## Scaling and reflow

- No fixed row heights (`min_h` only) — 2-line descriptions and larger text
  scales reflow.
- Row controls wrap below the label column when narrow (flex-wrap).
- Fonts follow the toolkit's font-size pipeline (theme base 14, mono 12);
  system font scaling applies through gpui's rem handling.
- Fractional scaling and narrow-window captures were taken 2026-10-30 (see
  the phase report addendum): the app renders natively at output scale 1.5
  (760×560 logical → 1140×840 physical, tokens intact), and both windows
  reflow at their spec minimum sizes (760×560 / 780×560) with wrapped labels
  and aligned controls.
