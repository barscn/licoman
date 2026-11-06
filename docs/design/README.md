# Quiet Control — Licoman Design System

**Design name:** Quiet Control. Personality: understated, reliable, precise,
discoverable, technically capable. Source of truth:
`01_DESIGN_SYSTEM.md` (author's document pack); this directory records how it
is realized in code and where deliberate deviations were taken.

## Documents

| File | Contents |
|---|---|
| [brand.md](brand.md) | Product name, voice, icon direction |
| [tokens.md](tokens.md) | Semantic color tokens, typography, spacing, radius, motion |
| [layout.md](layout.md) | Window geometry, regions, responsive behavior |
| [components.md](components.md) | Component anatomy and the gpui-kit mapping |
| [states.md](states.md) | The explicit visual state contract |
| [accessibility.md](accessibility.md) | Keyboard paths, focus, AccessKit semantics, contrast plan |
| [decisions.md](decisions.md) | Decision records and deviations |

## Implementation map

| Design concept | Code |
|---|---|
| Semantic tokens (dark/light) | `apps/licoman/src/theme.rs` (`DARK`/`LIGHT` palettes, `theme_config`) |
| Token→toolkit mapping | `theme.rs` → gpui-kit `ThemeConfig`, installed via `Theme::apply_config` |
| Product preferences | `apps/licoman/src/prefs.rs` (`$XDG_CONFIG_HOME/licoman/preferences.toml`) |
| Setting row anatomy | `apps/licoman/src/ui/mod.rs` (`setting_row`, `settings_group`) |
| Main window / palette / apply bar | `apps/licoman/src/ui/main_window.rs` |
| Applications panel | `apps/licoman/src/ui/apps_panel.rs` |
| Settings panel | `apps/licoman/src/ui/preferences_panel.rs` |
| Mock data (labeled as such) | `apps/licoman/src/mock.rs` |

Fresh installs launch **Dark**. Light and System are user-selectable in
`Settings → Appearance` (gear button or Ctrl+,). System resolves through the
desktop appearance signal and falls back to Dark, disclosed in Settings.
The preference file is only created after the user changes a preference.
