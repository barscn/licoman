# Components

Thin Licoman wrappers around verified gpui-kit primitives — no copied
internal state machines. Value-bearing controls never own committed state;
they read from and write through the shared `AppState`.

| Design name | Implementation | Built on |
|---|---|---|
| PageHeader / ScopeNavItem | `main_window.rs` (TitleBar row, `SidebarMenuItem.active`) | gpui-kit `TitleBar`, `Sidebar`, `SidebarMenuItem` |
| CategoryNav | scope items + footer environment block | `SidebarMenu` |
| SettingsGroup / SettingRow | `ui/mod.rs::settings_group`, `setting_row` | plain gpui divs over tokens |
| SettingControl | switch / select / slider / number / text / read-only value | gpui-kit `Switch`, `Select`, `Slider`, `NumberInput`, `Input` |
| SearchPalette | `main_window.rs::render_palette` | overlay div + `Input` |
| ApplyBar | `main_window.rs::render_apply_bar` | `Button` (primary/ghost, loading) |
| PermissionDialog | Failed/External/Permission actions | `window.open_alert_dialog` / `open_dialog` |
| SourceDetails | Applications "Configuration source → Details…" | `window.open_dialog` |
| StatusToast | (not yet exercised; lifecycle "Saved" is in-row) | gpui-kit notifications |
| AppListItem / support pill | `apps_window.rs` + `ui::support_tag` | `Tag` |
| EmptyState | palette "no results", apps "no filter match", unsupported app detail | plain divs (gpui-kit `Empty` evaluated, plain version used) |
| ThemeSelector | Preferences radio group | `RadioGroup`/`Radio` |

Row anatomy: label (14/500) + description (13) on the left; control
right-aligned in a 220 px column; state/validation message on a full-width
line below the row — never truncated beyond retrieval. The row uses flex-wrap
so the control reflows below the text when the window is narrow.

Control types exercised by the mock: boolean (Switch), enum (Select with the
actual mock options), number (NumberInput with step/min/max), text (Input),
slider (SliderState + value label), read-only (stable value + explanation —
never a disabled interactive control pretending to be editable).
