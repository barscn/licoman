//! Licoman — Linux Configuration Manager.
//! Application bootstrap: CLI, logging, preferences, theme install, windows.
//! All domain logic lives in `licoman-core`; all rendering in `licoman-ui`.

mod cli;
mod scale_demo;
mod writes_demo;

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

/// Filesystem-only probe for the Vulkan user-space stack: the loader
/// library and at least one installable-driver (ICD) manifest. Bounded and
/// side-effect free — a deeper failure (broken driver, device-less
/// context) still surfaces through the panic hook with the same pointer.
fn vulkan_precheck() -> Result<(), String> {
    if std::env::var_os("LICOMAN_SKIP_VULKAN_CHECK").is_some_and(|v| v == "1") {
        // Documented escape hatch for headless CI builds of the binary;
        // never set by normal runs.
        return Ok(());
    }
    let loader_candidates = [
        "/usr/lib/libvulkan.so.1",
        "/usr/lib/x86_64-linux-gnu/libvulkan.so.1",
        "/usr/lib64/libvulkan.so.1",
        "/usr/local/lib/libvulkan.so.1",
    ];
    let loader = loader_candidates.iter().find(|path| {
        std::fs::metadata(path)
            .map(|meta| meta.is_file())
            .unwrap_or(false)
    });
    let Some(loader) = loader else {
        return Err(
            "The Vulkan loader library (libvulkan.so.1) was not found.\n\
             Install the Vulkan loader package for your distribution —\n\
             e.g. 'vulkan-icd-loader' (Arch), 'libvulkan1' (Debian/Ubuntu),\n\
             'vulkan-loader' (Fedora) — and the matching driver package\n\
             (see the troubleshooting guide)."
                .to_string(),
        );
    };
    let icd_dirs = [
        "/usr/share/vulkan/icd.d",
        "/etc/vulkan/icd.d",
        "/usr/local/share/vulkan/icd.d",
    ];
    let home_icd = std::env::var_os("HOME")
        .map(|home| std::path::PathBuf::from(home).join(".local/share/vulkan/icd.d"));
    let mut driver_manifests = 0usize;
    let mut checked_dirs: Vec<String> = Vec::new();
    for dir in icd_dirs
        .iter()
        .map(std::path::PathBuf::from)
        .chain(home_icd)
    {
        checked_dirs.push(dir.display().to_string());
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.ends_with(".json") {
                    driver_manifests += 1;
                }
            }
        }
    }
    if driver_manifests == 0 {
        return Err(format!(
            "The Vulkan loader ({loader}) is installed, but no GPU driver\n\
             manifests (ICD JSON) were found in {}\n\
             Install the user-space driver for your GPU —\n\
             e.g. 'mesa-vulkan-radeon'/'vulkan-intel' (Arch),\n\
             'mesa-vulkan-drivers' (Debian/Ubuntu/Fedora) — or, for NVIDIA,\n\
             the vendor driver package.",
            checked_dirs.join(", ")
        )
        .to_string());
    }
    Ok(())
}

fn main() {
    licoman_ui::startup::mark_process_start();
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

    // Phase 09: a usable Vulkan user-space stack is a documented
    // prerequisite of the GPU renderer. Probe the loader and the driver
    // manifests BEFORE launching the GUI so an unsupported machine gets an
    // actionable diagnostic instead of a raw renderer failure (09 plan §2,
    // "Vulkan diagnostics"). This is a filesystem-only probe: no GPU calls,
    // no device creation, no output side effects.
    if let Err(problem) = vulkan_precheck() {
        licoman_core::log::error("licoman::main", "vulkan prerequisites missing", &[]);
        eprintln!(
            "licoman: the GPU rendering prerequisites are not met.\n\
             {problem}\n\
             Licoman renders through Vulkan and does not fall back to a\n\
             software path. Nothing was modified.\n\
             See the troubleshooting guide:\n\
             https://github.com/barscn/licoman/blob/main/docs/troubleshooting.md#vulkan"
        );
        std::process::exit(1);
    }

    // Even with the pre-probe passing, a broken driver or compositor
    // combination can fail during renderer construction. Attach a panic
    // hook that appends the troubleshooting pointer to graphics-shaped
    // panics so the failure is actionable, not raw (the panic itself still
    // surfaces — nothing is swallowed).
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = info.to_string().to_ascii_lowercase();
        if message.contains("vulkan")
            || message.contains("gpu")
            || message.contains("blade")
            || message.contains("surface")
            || message.contains("adapter")
        {
            eprintln!(
                "\nlicoman: this failure looks graphics-related. See the\n\
                 troubleshooting guide:\n\
                 https://github.com/barscn/licoman/blob/main/docs/troubleshooting.md#vulkan"
            );
        }
        previous_hook(info);
    }));

    // Load preferences before the runtime starts so the resolved theme is
    // installed before the first window paints — no bright first-frame flash.
    // A missing or corrupt file must not prevent startup: `load` falls back
    // to Dark with diagnostics for the Preferences window.
    let loaded = prefs::load();

    let mut registry = Registry::new();
    // The shipped catalog: built-in module manifests validated and indexed
    // at startup (Phase 04). LICOMAN_DEMO=empty exercises the no-providers
    // path: every scope shows the unavailable state instead of a blank
    // window. LICOMAN_DEMO=reads wires a fixture-backed backend router so
    // the Phase 05 read paths render deterministically (visual verification
    // only — the live run uses the real environment).
    let demo_name = std::env::var("LICOMAN_DEMO").unwrap_or_default();
    // Every reads* demo uses the fixture router — a demo must never read
    // (or write) live host state (caught by the Phase 06 re-verification:
    // `reads-apps`/`reads-power` silently used the live environment).
    let reads_demo = matches!(demo_name.as_str(), "reads" | "reads-power" | "reads-apps");
    let writes_demo = matches!(
        demo_name.as_str(),
        "writes" | "writes-conflict" | "writes-permission" | "writes-light"
    );
    let writes_env = if writes_demo {
        Some(writes_demo::WritesEnv::create().expect("fixture write environment"))
    } else {
        None
    };
    let module_provider = if demo_name != "empty" {
        let is_scale_demo = demo_name == "scale";
        let module_registry = if is_scale_demo {
            // Synthetic scale catalog (Phase 08 performance gates): purely
            // mock-backed generated manifests — 100 shared modules x 10
            // settings + 120 application modules x 2 settings.
            let generated = scale_demo::manifests(100, 10, 120, 2);
            licoman_core::modules::ModuleRegistry::load(generated.into_iter().map(
                |(name, text)| {
                    (
                        licoman_core::modules::registry::ModuleSource::Builtin(
                            name.leak() as &'static str
                        ),
                        text,
                    )
                },
            ))
        } else {
            licoman_core::modules::ModuleRegistry::load_builtin()
        };
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
        let provider = std::sync::Arc::new(if reads_demo {
            let fixtures =
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
            licoman_core::modules::ModuleProvider::with_router(
                module_registry.clone(),
                licoman_core::read::BackendRouter::new(
                    licoman_core::read::gsettings::GSettingsReadBackend::for_paths(
                        vec![fixtures.join("discovery/schemas")],
                        Some(fixtures.join("gsettings/dconf/user")),
                        vec![fixtures.join("gsettings/dconf/system/site")],
                    ),
                    licoman_core::read::file::FileReadBackend::for_dirs(
                        fixtures.join("alacritty/config"),
                        fixtures.join("alacritty/home"),
                        vec![fixtures.join("alacritty/etc")],
                    ),
                    licoman_core::read::dbus::DBusReadBackend::with_client(std::sync::Arc::new(
                        licoman_core::read::dbus::AbsentServiceClient,
                    )),
                    licoman_core::read::xfconf::XfconfReadBackend::with_client(
                        std::sync::Arc::new(licoman_core::read::xfconf::AbsentXfconfClient),
                    ),
                ),
            )
        } else if writes_demo {
            writes_env
                .as_ref()
                .expect("writes env")
                .provider(module_registry.clone())
        } else if is_scale_demo {
            // Mock-only catalog: no live reads of any kind. Absent clients
            // make the choice explicit.
            let fixtures =
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
            licoman_core::modules::ModuleProvider::with_router(
                module_registry.clone(),
                licoman_core::read::BackendRouter::new(
                    licoman_core::read::gsettings::GSettingsReadBackend::for_paths(
                        vec![fixtures.join("discovery/schemas")],
                        None,
                        vec![],
                    ),
                    licoman_core::read::file::FileReadBackend::for_dirs(
                        fixtures.join("file-config"),
                        fixtures.join("file-home"),
                        vec![],
                    ),
                    licoman_core::read::dbus::DBusReadBackend::with_client(std::sync::Arc::new(
                        licoman_core::read::dbus::AbsentServiceClient,
                    )),
                    licoman_core::read::xfconf::XfconfReadBackend::with_client(
                        std::sync::Arc::new(licoman_core::read::xfconf::AbsentXfconfClient),
                    ),
                ),
            )
        } else {
            // Phase 07 (D49): the live run carries the real write engine.
            // Nothing writes without the user's explicit Apply/Confirm; the
            // gates stay closed for non-GNOME sessions and unverified
            // application versions.
            licoman_core::modules::ModuleProvider::live(module_registry.clone())
        });
        registry.register(Box::new(licoman_core::modules::ModuleProvider::clone(
            &provider,
        )));
        Some(provider)
    } else {
        None
    };
    // The optional Hyprland read-only diagnostics (07 plan section) — live
    // runs only: demo fixtures must never read live host state, so demo
    // processes do not register this provider. The handle is shared with
    // the AppState read pass so its ids route to the right provider.
    let hyprland_diagnostics = if demo_name.is_empty() {
        let provider = std::sync::Arc::new(licoman_core::hyprland::HyprlandDiagnostics::new());
        registry.register(Box::new(
            licoman_core::hyprland::HyprlandDiagnostics::clone(&provider),
        ));
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
            if demo.starts_with("light") || demo == "writes-light" {
                initial_mode = gpui_kit::component::theme::ThemeMode::Light;
            }
            theme::install(cx, initial_mode);

            cx.set_global(WindowRegistry::default());
            let app = cx.new(|_| {
                let mut state = AppState::new(
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
                );
                // The Hyprland diagnostics provider serves its own ids in the
                // background read pass (registered live-only below).
                if let Some(handle) = hyprland_diagnostics.clone() {
                    state.add_extra_reader(handle);
                }
                state
            });

            // Environment discovery (Phase 03). Demo fixture names replace
            // discovery with a deterministic snapshot; every other run
            // discovers the real environment off the UI thread and watches
            // the application directories for changes. The `reads` demo
            // keeps the arch-hyprland fixture environment but runs real
            // read passes against the fixture-backed router.
            let demo_env = if demo == "reads"
                || demo == "reads-power"
                || demo == "reads-apps"
                || demo == "scale"
                || demo.starts_with("writes")
            {
                "arch-hyprland"
            } else {
                demo.as_str()
            };
            if licoman_core::discovery::demo_fixtures::demo_snapshot(demo_env).is_some() {
                app.update(cx, |state, cx| {
                    // The flag must be set before the demo environment
                    // ingests: the ingest itself schedules the first read
                    // pass, and the demo-locked guard consults it.
                    if demo.starts_with("reads") || demo.starts_with("writes") {
                        state.set_reads_fixture_backed();
                    }
                    state.load_demo_environment(demo_env, cx);
                    if demo == "reads-power" {
                        state.dispatch(Command::SelectCategory(Some("Power".into())), cx);
                    }
                    if demo.starts_with("writes") {
                        // The demo rows live in Shared/Power (fake service)
                        // and Distribution (permission-held).
                        if demo == "writes-permission" {
                            state.dispatch(Command::SelectScope(Scope::Distribution), cx);
                        } else {
                            state.dispatch(Command::SelectCategory(Some("Power".into())), cx);
                        }
                    }
                });
            } else {
                app.update(cx, |state, cx| {
                    state.ensure_environment_watcher(cx);
                    state.start_discovery(cx);
                });
                // Capture affordance for LIVE-state screenshots (no demo
                // fixture, no locking — discovery and reads stay live):
                // LICOMAN_CAPTURE_NAV="Scope/Category" navigates once the
                // first discovery stages have published. Navigation only;
                // it never changes setting state.
                let nav = std::env::var("LICOMAN_CAPTURE_NAV").unwrap_or_default();
                if let Some((scope, category)) = nav.split_once('/') {
                    // "Application/<app-id>" opens the Applications window and
                    // selects the app; other scopes navigate the main window.
                    let app_for_nav = app.clone();
                    let application_scope = scope == "Application";
                    let app_id = category.to_string();
                    let category = category.to_string();
                    let scope = match scope {
                        "Shared" => Scope::Shared,
                        "Desktop" => Scope::Desktop,
                        "Distribution" => Scope::Distribution,
                        _ => Scope::Shared,
                    };
                    cx.spawn(async move |cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(4500))
                            .await;
                        if application_scope {
                            // Same shape as the app-detail demo hook: the
                            // env names a MODULE id ("Application/app.mpv");
                            // the identity resolves from the merged app list
                            // once discovery published it, SelectApp runs in
                            // its own update (the apps window reads AppState
                            // at creation — a read inside the same update
                            // would panic), and the apps window is notified
                            // explicitly (a programmatic dispatch has no
                            // click listener).
                            // AsyncApp::update returns the closure's value.
                            let (handle, apps_window) = cx.update(|app| {
                                windows::open_apps_window_returning_entity(&app_for_nav, app)
                            });
                            cx.update(|app| {
                                app.global_mut::<licoman_ui::app_state::WindowRegistry>()
                                    .set_handle(licoman_core::WindowRole::Applications, handle);
                            });
                            app_for_nav.update(cx, |state, cx| {
                                let identity = state.select_identity_for_module(&app_id.clone());
                                licoman_core::log::info(
                                    "licoman::capture",
                                    "capture navigation dispatched",
                                    &[
                                        ("module", app_id.clone()),
                                        ("identity", identity.clone().unwrap_or_default()),
                                    ],
                                );
                                state.dispatch(Command::SelectApp { id: identity }, cx);
                            });
                            apps_window.update(cx, |_this, cx| {
                                cx.notify();
                            });
                        } else {
                            app_for_nav.update(cx, |state, cx| {
                                licoman_core::log::info(
                                    "licoman::capture",
                                    "capture navigation dispatched",
                                    &[("category", category.clone())],
                                );
                                state.dispatch(Command::SelectScope(scope), cx);
                                state.dispatch(Command::SelectCategory(Some(category.clone())), cx);
                                // Navigation commands report settings_changed:
                                // false, so the windows must be notified
                                // explicitly (interactive clicks do this through
                                // their listeners; a programmatic hook does not).
                                cx.notify();
                            });
                        }
                    })
                    .detach();
                }
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
                    .detach()
                }
                "writes" | "writes-conflict" | "writes-light" => {
                    // Stage one service-backed edit (fake power-profiles
                    // service) and one file-backed edit (temp fixture copy)
                    // once the read pass published. The apply bar in the main
                    // window shows both — including edits staged from the
                    // Applications window (both windows share the store).
                    let app_for_stage = app.clone();
                    let demo_kind = demo.clone();
                    cx.spawn(async move |cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(4000))
                            .await;
                        app_for_stage.update(cx, |state, cx| {
                            state.dispatch(
                                Command::Edit {
                                    id: "shared.power.profile".into(),
                                    value: licoman_core::SettingValue::Choice("performance".into()),
                                },
                                cx,
                            );
                            state.dispatch(
                                Command::Edit {
                                    id: "app.alacritty.font_size".into(),
                                    value: licoman_core::SettingValue::Float(14.0),
                                },
                                cx,
                            );
                        });
                        if demo_kind == "writes-conflict" {
                            // The service state changes behind Licoman's back
                            // (the fake daemon holds a different profile) and
                            // the staged row learns about it: it shows the
                            // conflict state with Reload/Compare — never an
                            // overwrite. The service is genuinely mutated
                            // first; the store notification follows.
                            if let Some(env) = writes_env.as_ref() {
                                env.inject_external_service_change();
                            }
                            app_for_stage.update(cx, |state, cx| {
                                state.store.simulate_external_change(
                                    "shared.power.profile",
                                    licoman_core::SettingValue::Choice("power-saver".into()),
                                );
                                cx.emit(licoman_ui::app_state::AppEvent::SettingsChanged);
                            });
                        }
                    })
                    .detach();
                }
                "writes-permission" => {
                    // The permission-held row ("System updates" — the catalog
                    // keeps it read-only-controlled, so an edit attempt is a
                    // NoOp) is shown in the distinct Permission-required
                    // state; the authorize dialog explains the deny-by-default
                    // broker. Edit-driven refusals reach this same phase
                    // through the store when the control is editable.
                    let app_for_stage = app.clone();
                    cx.spawn(async move |cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(4000))
                            .await;
                        app_for_stage.update(cx, |state, _cx| {
                            state
                                .store
                                .simulate_permission_required("distro.packages.updates");
                        });
                        app_for_stage.update(cx, |state, cx| {
                            state.dispatch(Command::SelectCategory(Some("Packages".into())), cx);
                            cx.emit(licoman_ui::app_state::AppEvent::SettingsChanged);
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
            if demo == "reads" {
                // After the fixture read pass published, open the Source
                // details for a gsettings-backed row (visual verification of
                // the provenance disclosure).
                let main_for_details = main_window.downgrade();
                cx.spawn(async move |cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(2500))
                        .await;
                    let _ = main_for_details.update_in(cx, |this, window, cx| {
                        this.demo_open_source_details(
                            "shared.appearance.gsettings_color_scheme",
                            window,
                            cx,
                        );
                    });
                })
                .detach();
            }
            if demo == "apps"
                || demo == "light-apps"
                || demo == "all-apps"
                || demo == "app-detail"
                || demo == "reads-apps"
            {
                let (handle, apps_window) = windows::open_apps_window_returning_entity(&app, cx);
                if demo == "all-apps" {
                    apps_window.update(cx, |this, _cx| this.demo_show_all());
                }
                cx.global_mut::<WindowRegistry>()
                    .set_handle(licoman_core::WindowRole::Applications, handle);
                if demo == "app-detail" || demo == "reads-apps" {
                    // Select an installed module-backed app so the detail
                    // pane shows the registry-driven tabs and rows. Wait for
                    // discovery to publish the merged list first; SelectApp
                    // changes no setting state, so the window is notified
                    // explicitly (interactive clicks do this in their
                    // listeners; a demo hook has no listener). The reads
                    // variant selects Alacritty: its rows are read from the
                    // fixture TOML config through the file backend.
                    let target = if demo == "reads-apps" {
                        "app.alacritty"
                    } else {
                        "app.mpv"
                    };
                    let app_for_detail = app.clone();
                    let window_for_detail = apps_window.clone();
                    cx.spawn(async move |cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(4000))
                            .await;
                        app_for_detail.update(cx, |state, cx| {
                            let identity = state.select_identity_for_module(target);
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
