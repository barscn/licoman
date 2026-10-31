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

/// Read-only module linter (04_MODULE_SCHEMA_AND_REGISTRY.md §5):
/// `licoman module validate <path>...`. Parses and validates each manifest
/// (or every `*.toml` directly inside a directory, sorted), prints located
/// diagnostics and — when a discovery snapshot is available — the module's
/// applicability against this environment. Never writes anything.
fn run_module_validate(paths: &[String]) -> i32 {
    licoman_core::log::init_from_env(licoman_core::log::Level::Warn);
    let mut sources: Vec<(licoman_core::modules::ModuleSource, String)> = Vec::new();
    let mut failures = 0usize;

    for raw in paths {
        let path = std::path::PathBuf::from(raw);
        if path.is_dir() {
            let mut files: Vec<std::path::PathBuf> = match std::fs::read_dir(&path) {
                Ok(entries) => entries
                    .filter_map(|entry| entry.ok())
                    .map(|entry| entry.path())
                    .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "toml"))
                    .collect(),
                Err(error) => {
                    eprintln!("{raw}: error: cannot read directory ({error})");
                    failures += 1;
                    continue;
                }
            };
            files.sort();
            for file in files {
                read_manifest_source(file, &mut sources, &mut failures);
            }
        } else {
            read_manifest_source(path, &mut sources, &mut failures);
        }
    }

    // Environment snapshot for applicability reporting (best effort; a
    // missing graphical session only means "pending").
    let snapshot = {
        let sources_env = licoman_core::discovery::DiscoverySources::from_environment();
        licoman_core::discovery::discover(
            &sources_env,
            &licoman_core::discovery::ScanControl::new(),
            &|_| {},
        )
        .0
    };

    let registry = licoman_core::modules::ModuleRegistry::load(sources);
    for rejected in registry.rejected() {
        failures += 1;
        for issue in &rejected.issues {
            println!("{}", issue.render(rejected.source.display().as_str()));
        }
    }
    for module in registry.modules() {
        let applicability = licoman_core::modules::compat::evaluate(
            &module.def,
            Some(&licoman_core::modules::EnvironmentFacts::from_snapshot(
                &snapshot,
            )),
        );
        match applicability.state {
            licoman_core::modules::compat::ApplicabilityState::Applicable => println!(
                "{}: ok — {} settings; applicable to this environment",
                module.source.display(),
                module.def.settings.len()
            ),
            licoman_core::modules::compat::ApplicabilityState::Inapplicable => println!(
                "{}: ok — {} settings; NOT applicable here ({})",
                module.source.display(),
                module.def.settings.len(),
                applicability
                    .reasons
                    .iter()
                    .map(|r| r.message.clone())
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            licoman_core::modules::compat::ApplicabilityState::Pending => println!(
                "{}: ok — {} settings; applicability pending",
                module.source.display(),
                module.def.settings.len()
            ),
        }
    }

    if failures == 0 { 0 } else { 1 }
}

fn read_manifest_source(
    path: std::path::PathBuf,
    sources: &mut Vec<(licoman_core::modules::ModuleSource, String)>,
    failures: &mut usize,
) {
    match std::fs::read_to_string(&path) {
        Ok(text) => sources.push((licoman_core::modules::ModuleSource::File(path), text)),
        Err(error) => {
            eprintln!("{}: error: cannot read file ({error})", path.display());
            *failures += 1;
        }
    }
}

/// Print the JSON Schema generated from the canonical module types
/// (`licoman module schema`).
fn run_module_schema() {
    let schema = schemars::schema_for!(licoman_core::modules::ModuleDefinition);
    println!(
        "{}",
        serde_json::to_string_pretty(&schema).expect("schema serializes")
    );
}

/// Headless environment diagnostics: run read-only discovery and print the
/// redacted export. No window, no preferences, no writes.
fn run_diagnostics() {
    licoman_core::log::init_from_env(licoman_core::log::Level::Warn);
    let sources = licoman_core::discovery::DiscoverySources::from_environment();
    let home = sources.env.get("HOME").map(String::as_str);
    let (snapshot, _status, timings) = licoman_core::discovery::discover(
        &sources,
        &licoman_core::discovery::ScanControl::new(),
        &|_| {},
    );
    print!(
        "{}",
        licoman_core::discovery::export::diagnostics_text(&snapshot, home)
    );
    licoman_core::log::info(
        "licoman::diagnostics",
        "diagnostics export completed",
        &[("total_us", timings.total_micros.to_string())],
    );
}

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
        cli::Parsed::Diagnostics => {
            run_diagnostics();
            std::process::exit(0);
        }
        cli::Parsed::ModuleValidate(paths) => {
            std::process::exit(run_module_validate(&paths));
        }
        cli::Parsed::ModuleSchema => {
            run_module_schema();
            std::process::exit(0);
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
    // The shipped catalog: built-in module manifests validated and indexed
    // at startup (Phase 04). LICOMAN_DEMO=empty exercises the no-providers
    // path: every scope shows the unavailable state instead of a blank
    // window.
    let module_provider = if std::env::var("LICOMAN_DEMO").ok().as_deref() != Some("empty") {
        let module_registry = licoman_core::modules::ModuleRegistry::load_builtin();
        for rejected in module_registry.rejected() {
            licoman_core::log::error(
                "licoman::modules",
                "built-in manifest rejected",
                &[
                    ("source", rejected.source.display()),
                    ("issues", rejected.issues.len().to_string()),
                ],
            );
        }
        let module_registry = std::sync::Arc::new(std::sync::Mutex::new(module_registry));
        let provider = std::sync::Arc::new(licoman_core::modules::ModuleProvider::new(
            module_registry.clone(),
        ));
        registry.register(Box::new(licoman_core::modules::ModuleProvider::clone(
            &provider,
        )));
        Some(provider)
    } else {
        None
    };
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
            let app = cx.new(|_| {
                AppState::new(
                    store,
                    module_provider.unwrap_or_else(|| {
                        // Demo "empty" has no module provider; the UI only
                        // reads through it, so an empty registry is safe.
                        std::sync::Arc::new(licoman_core::modules::ModuleProvider::new(
                            std::sync::Arc::new(std::sync::Mutex::new(
                                licoman_core::modules::ModuleRegistry::load([]),
                            )),
                        ))
                    }),
                )
            });

            // Environment discovery (Phase 03). Demo fixture names replace
            // discovery with a deterministic snapshot; every other run
            // discovers the real environment off the UI thread and watches
            // the application directories for changes.
            if licoman_core::discovery::demo_fixtures::demo_snapshot(&demo).is_some() {
                app.update(cx, |state, cx| {
                    state.load_demo_environment(&demo, cx);
                });
            } else {
                app.update(cx, |state, cx| {
                    state.ensure_environment_watcher(cx);
                    state.start_discovery(cx);
                });
            }

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
                "dirty" => {
                    // Stage the edit once discovery finished: before that the
                    // rows are pending-unavailable and correctly refuse edits.
                    let app_for_dirty = app.clone();
                    cx.spawn(async move |cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(4000))
                            .await;
                        app_for_dirty.update(cx, |state, cx| {
                            state.dispatch(
                                Command::Edit {
                                    id: "desktop.session.gaps".into(),
                                    value: licoman_core::SettingValue::Bool(false),
                                },
                                cx,
                            );
                        });
                    })
                    .detach();
                }
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
                main_window.update(cx, |this, cx| this.demo_open_palette(cx));
            }
            if demo == "apps" || demo == "light-apps" || demo == "all-apps" || demo == "app-detail"
            {
                let (handle, apps_window) = windows::open_apps_window_returning_entity(&app, cx);
                if demo == "all-apps" {
                    apps_window.update(cx, |this, _cx| this.demo_show_all());
                }
                cx.global_mut::<WindowRegistry>()
                    .set_handle(licoman_core::WindowRole::Applications, handle);
                if demo == "app-detail" {
                    // Select an installed module-backed app so the detail
                    // pane shows the registry-driven tabs and rows. Wait for
                    // discovery to publish the merged list first; SelectApp
                    // changes no setting state, so the window is notified
                    // explicitly (interactive clicks do this in their
                    // listeners; a demo hook has no listener).
                    let app_for_detail = app.clone();
                    let window_for_detail = apps_window.clone();
                    cx.spawn(async move |cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(4000))
                            .await;
                        app_for_detail.update(cx, |state, cx| {
                            let identity = state.select_identity_for_module("app.mpv");
                            state.dispatch(Command::SelectApp { id: identity }, cx);
                        });
                        window_for_detail.update(cx, |_this, cx| {
                            cx.notify();
                        });
                    })
                    .detach();
                }
            }
            if demo == "prefs" {
                let handle = windows::open_preferences_window(&app, cx);
                cx.global_mut::<WindowRegistry>()
                    .set_handle(licoman_core::WindowRole::Preferences, handle);
            }
        });
}
