# Tokens

All values are design tokens, not gpui-kit field names. The single mapping
from Licoman tokens to the pinned gpui-kit theme API lives in
`apps/licoman/src/theme.rs`; raw hex codes appear nowhere else.

## Color

| Semantic token | Dark (**default**) | Light | Usage |
|---|---|---|---|
| `canvas` | `#0E1117` | `#F4F6FA` | Main background |
| `sidebar` | `#141922` | `#EBEFF5` | Navigation pane, title bar |
| `surface` | `#171D27` | `#FFFFFF` | Form/list surface, popovers |
| `raised` | `#1D2633` | `#F9FAFD` | Elevation, controls, buttons |
| `hover` | `#253143` | `#E7EDF7` | Hover / active press |
| `selected` | `#263753` | `#E2ECFC` | Selected nav / list rows |
| `border` | `#344155` | `#D8E0EA` | Hairlines, input borders |
| `text.primary` | `#EDF2F8` | `#1A2635` | Titles, setting labels |
| `text.secondary` | `#B0BDCE` | `#4D5E73` | Descriptions |
| `text.muted` | `#97A6B9` | `#5A6B81` | Metadata |
| `accent` | `#8AB4FF` | `#255FD4` | Primary action, selection, focus |
| `on_accent` | `#102035` | `#FFFFFF` | Label on accent fill |
| `success` | `#70D0A9` | `#167C5C` | Applied status only |
| `warning` | `#F2C36C` | `#945A0B` | Pending / restart / conflict |
| `danger` | `#FF9399` | `#B83243` | Error / destructive |

Derived tones (not in the spec table, required by toolkit controls):
`accent_hover`, `accent_active`, `info`, `scrollbar_thumb` — one per mode, in
`theme.rs`.

**Contrast status:** dark-mode body text (`#EDF2F8` on `#0E1117`) and
light-mode body text (`#1A2635` on `#F4F6FA`) clear 4.5:1 by a wide margin;
the accent-on-accent pairs (`on_accent` fills) and secondary/muted text
combinations are scheduled for a measured WCAG pass in Phase 09
(release quality) together with high-contrast mode. Rendered evidence:
`docs/screenshots/*-dark.png`, `*-light.png`.

## Typography

System UI sans via gpui's `.SystemUIFont` resolution (fontconfig on Linux);
monospace falls back through DejaVu Sans Mono and friends. **No fonts are
bundled.** Base size 14, mono 12 (set through the theme config).

| Role | Size (px) | Weight |
|---|---:|---|
| Scope/window heading | 24 | 600 |
| Page/application heading | 21 | 600 |
| Group heading | 16 | 600 |
| Setting label | 14 | 500 |
| Body/description | 13 | 400 |
| Supporting metadata | 12 | 400 |

Text sizes are set per element from `theme::metrics`, never fixed row heights:
rows use `min_h` so two-line descriptions and larger text scales cannot clip.

## Spacing, shape, motion

```text
Spacing: 0, 2, 4, 8, 12, 16, 20, 24, 32, 40, 48
Radius:  small 5, medium 8 (theme radius), large 12 (radius_lg), xl 16 (= md*2)
Border:  1 logical px, tinted (token `border`)
Focus:   2 px ring outside the control (toolkit `focus_ring: true`)
Icons:   16 inline / 18 nav / 20 app / 24 empty state (Lucide set via gpui-kit assets)
Rows:    setting row min height 60, control column 220 (spec: 200–260)
Motion:  toolkit defaults (~100–180 ms); nothing depends on animation to
         communicate state; no perpetual background animation
```
