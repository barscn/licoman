//! Licoman — Linux Configuration Manager.
//! Phase 01 executable: design-system shell on mock data.

mod actions;
mod app_state;
mod mock;
mod prefs;
mod theme;
mod ui;
mod windows;

use gpui_kit::{App, AppContext, KeyBinding, application, init};

use crate::actions::*;
use crate::app_state::{AppState, WindowRegistry};
use crate::prefs::AppearanceMode;

fn main() {
    // Load preferences before the runtime starts so the resolved theme is
    // installed before the first window paints — no bright first-frame flash.
    // A missing or corrupt file must not prevent startup: `load` falls back
    // to Dark with diagnostics for the Preferences window.
    let loaded = prefs::load();

    application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx: &mut App| {
            // This must be called before using any GPUI Component features.
            init(cx);

            // Visual-verification affordance, documented in docs/handoffs:
            // LICOMAN_DEMO=palette|apps|prefs|light|light-apps|failed|distro
            let demo = std::env::var("LICOMAN_DEMO").unwrap_or_default();

            let mut initial_mode = match loaded.preferences.theme {
                AppearanceMode::Dark => gpui_kit::component::theme::ThemeMode::Dark,
                AppearanceMode::Light => gpui_kit::component::theme::ThemeMode::Light,
                // System tracks the desktop signal; when it is unavailable this
                // resolves to Dark (disclosed in Preferences).
                AppearanceMode::System => app_state::system_theme_mode(cx.window_appearance()),
            };
            if demo.starts_with("light") {
                initial_mode = gpui_kit::component::theme::ThemeMode::Light;
            }
            theme::install(cx, initial_mode);

            cx.set_global(WindowRegistry::default());
            let app = cx.new(|_| AppState::new(loaded.preferences, loaded.diagnostics));

            match demo.as_str() {
                "failed" => app.update(cx, |state, _| {
                    for setting in &mut state.settings {
                        if setting.simulate_failure {
                            setting.status = crate::mock::RowStatus::Failed {
                                reason: "The mock provider rejected this write.".into(),
                            };
                        }
                    }
                }),
                "distro" => app.update(cx, |state, _| {
                    state.selected_scope = crate::mock::Scope::Distribution;
                }),
                _ => {}
            }

            cx.bind_keys([
                KeyBinding::new("ctrl-k", OpenSearch, Some("MainWindow")),
                KeyBinding::new("ctrl-f", FocusSearch, Some("MainWindow")),
                KeyBinding::new("ctrl-k", OpenSearch, Some("AppsWindow")),
                KeyBinding::new("ctrl-f", FocusSearch, Some("AppsWindow")),
                KeyBinding::new("escape", DismissPalette, Some("SearchPalette")),
                KeyBinding::new("down", PaletteNext, Some("SearchPalette")),
                KeyBinding::new("up", PalettePrev, Some("SearchPalette")),
                KeyBinding::new("ctrl-,", OpenPreferences, None),
                KeyBinding::new("ctrl-q", Quit, None),
            ]);
            cx.on_action(|_: &Quit, cx| cx.quit());

            let (_, main_window) = windows::open_main_window(&app, cx);

            if demo == "palette" {
                main_window.update(cx, |this, _cx| this.demo_open_palette());
            }
            if demo == "apps" || demo == "light-apps" {
                let handle = windows::open_apps_window(&app, cx);
                cx.global_mut::<WindowRegistry>().apps_window = Some(handle);
            }
            if demo == "prefs" {
                let handle = windows::open_preferences_window(&app, cx);
                cx.global_mut::<WindowRegistry>().preferences_window = Some(handle);
            }
        });
}
