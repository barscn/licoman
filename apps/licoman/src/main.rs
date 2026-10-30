//! Licoman — Linux Configuration Manager.
//! Application bootstrap: CLI, logging, preferences, theme install, windows.
//! All domain logic lives in `licoman-core`; all rendering in `licoman-ui`.

mod cli;

use gpui_kit::{App, AppContext, KeyBinding, application, init};
use licoman_core::prefs;
use licoman_core::{Command, Registry, Scope, Store};
use licoman_ui::actions::*;
use licoman_ui::app_state::{AppState, WindowRegistry};
use licoman_ui::{theme, windows};

fn main() {
    match cli::parse(std::env::args().skip(1)) {
        cli::Parsed::PrintAndSucceed(text) => {
            println!("{text}");
            std::process::exit(0);
        }
        cli::Parsed::Fail(text) => {
            eprintln!("{text}");
            std::process::exit(2);
        }
        cli::Parsed::Run => {}
    }

    licoman_core::log::init_from_env(licoman_core::log::Level::Warn);

    // A graphical session is required. Failing here with a readable
    // diagnostic is the "unsupported environment" contract — never a blank
    // or broken window (02_FOUNDATION.md §3).
    if std::env::var_os("WAYLAND_DISPLAY").is_none()
        && std::env::var_os("WAYLAND_SOCKET").is_none()
        && std::env::var_os("DISPLAY").is_none()
    {
        licoman_core::log::error("licoman::main", "no graphical session detected", &[]);
        eprintln!(
            "licoman: no graphical session detected.\n\
             Set WAYLAND_DISPLAY (Wayland) or DISPLAY (X11) and start Licoman\n\
             from inside a desktop session. Nothing was modified."
        );
        std::process::exit(1);
    }

    // Load preferences before the runtime starts so the resolved theme is
    // installed before the first window paints — no bright first-frame flash.
    // A missing or corrupt file must not prevent startup: `load` falls back
    // to Dark with diagnostics for the Preferences window.
    let loaded = prefs::load();

    let mut registry = Registry::new();
    // LICOMAN_DEMO=empty exercises the no-providers path: every scope shows
    // the unavailable state instead of a blank window.
    if std::env::var("LICOMAN_DEMO").ok().as_deref() != Some("empty") {
        registry.register(Box::new(licoman_core::mock::MockProvider::standard()));
    }
    let store = Store::new(registry, loaded.clone(), prefs::preferences_path());

    application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx: &mut App| {
            // This must be called before using any GPUI Component features.
            init(cx);

            // Visual-verification affordance, documented in docs/handoffs:
            // LICOMAN_DEMO=palette|apps|prefs|light|light-apps|failed|distro|dirty
            let demo = std::env::var("LICOMAN_DEMO").unwrap_or_default();

            let mut initial_mode = match loaded.preferences.theme {
                prefs::AppearanceMode::Dark => gpui_kit::component::theme::ThemeMode::Dark,
                prefs::AppearanceMode::Light => gpui_kit::component::theme::ThemeMode::Light,
                // System tracks the desktop signal; when it is unavailable this
                // resolves to Dark (disclosed in Preferences).
                prefs::AppearanceMode::System => {
                    licoman_ui::app_state::system_theme_mode(cx.window_appearance())
                }
            };
            if demo.starts_with("light") {
                initial_mode = gpui_kit::component::theme::ThemeMode::Light;
            }
            theme::install(cx, initial_mode);

            cx.set_global(WindowRegistry::default());
            let app = cx.new(|_| AppState::new(store));

            match demo.as_str() {
                "failed" => app.update(cx, |state, cx| {
                    state.store.simulate_failed("shared.network.dns");
                    cx.emit(licoman_ui::app_state::AppEvent::SettingsChanged);
                }),
                "distro" => app.update(cx, |state, cx| {
                    state.dispatch(Command::SelectScope(Scope::Distribution), cx);
                }),
                "desktop" => app.update(cx, |state, cx| {
                    state.dispatch(Command::SelectScope(Scope::Desktop), cx);
                }),
                "dirty" => app.update(cx, |state, cx| {
                    state.dispatch(
                        Command::Edit {
                            id: "desktop.session.gaps".into(),
                            value: licoman_core::SettingValue::Bool(false),
                        },
                        cx,
                    );
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

            let (main_handle, main_window) = windows::open_main_window(&app, cx);
            cx.global_mut::<WindowRegistry>()
                .set_handle(licoman_core::WindowRole::Main, main_handle);

            if demo == "palette" {
                main_window.update(cx, |this, _cx| this.demo_open_palette());
            }
            if demo == "apps" || demo == "light-apps" {
                let handle = windows::open_apps_window(&app, cx);
                cx.global_mut::<WindowRegistry>()
                    .set_handle(licoman_core::WindowRole::Applications, handle);
            }
            if demo == "prefs" {
                let handle = windows::open_preferences_window(&app, cx);
                cx.global_mut::<WindowRegistry>()
                    .set_handle(licoman_core::WindowRole::Preferences, handle);
            }
        });
}
