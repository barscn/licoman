//! The view adapter: owns the core [`Store`] inside a GPUI entity, translates
//! UI intent into typed commands, emits observable events, drives the visible
//! apply interval, applies Licoman themes to every window, and models
//! single-instance window behavior with independent navigation histories.
//!
//! Phase 04 adds the module-registry handle: applicability is recomputed
//! whenever a discovery snapshot is ingested (staged discovery, demo fixture
//! or refresh), and the store reloads **only when the set of available
//! settings actually changed** — preserving navigation and staged edits
//! otherwise (04_MODULE_SCHEMA_AND_REGISTRY.md §6).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{AnyWindowHandle, App, Context, EventEmitter, Global};
use licoman_core::command::DispatchReport;
use licoman_core::discovery::{
    AppListEntry, DiscoveryRun, DiscoverySources, EnvironmentSnapshot, ScanControl, ScanStage,
    ScanStatus, build_app_list,
};
use licoman_core::modules::ModuleProvider;
use licoman_core::prefs::AppearanceMode;
use licoman_core::window_sessions::{OpenDecision, WindowSessions};
use licoman_core::{Command, SettingPhase, Store, WindowRole};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppEvent {
    /// Setting descriptors, models or selections changed.
    SettingsChanged,
    /// Licoman preferences (theme) changed.
    PreferencesChanged,
    /// Environment discovery progressed (loading, partial or ready).
    EnvironmentChanged,
    /// Module applicability changed (a reload published new availability).
    ModulesChanged,
}

/// Where the discovery cache stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanState {
    /// No discovery has run yet (or a demo fixture replaced discovery).
    Idle,
    /// A scan is running; `stage` is what is still to come.
    Loading { stage: ScanStage },
    /// The latest scan completed; the snapshot is current.
    Ready,
    /// The latest scan was cancelled (a Refresh always follows with a new
    /// scan; this state is transient).
    Cancelled,
}

/// The discovery cache owned by the adapter. Snapshot plus the derived,
/// deterministic app/module merge — safe metadata only.
pub struct EnvironmentState {
    snapshot: Option<Arc<EnvironmentSnapshot>>,
    app_list: Vec<AppListEntry>,
    scan: ScanState,
    control: Arc<ScanControl>,
    /// Bumped per started scan; stale tasks must not publish.
    generation: u64,
    /// Demo fixtures replace discovery entirely (visual verification).
    demo_locked: bool,
}

impl Default for EnvironmentState {
    fn default() -> Self {
        Self {
            snapshot: None,
            app_list: Vec::new(),
            scan: ScanState::Idle,
            control: Arc::new(ScanControl::new()),
            generation: 0,
            demo_locked: false,
        }
    }
}

impl EnvironmentState {
    fn ingest(&mut self, run: &DiscoveryRun, scan: ScanState) {
        self.snapshot = Some(Arc::new(run.snapshot()));
        self.scan = scan;
    }
}

/// GPUI-side window registry: real window handles per role, with the
/// open/focus/history decisions delegated to the core
/// [`WindowSessions`] model (unit tested headless).
#[derive(Default)]
pub struct WindowRegistry {
    pub sessions: WindowSessions,
    handles: HashMap<WindowRole, AnyWindowHandle>,
}

impl Global for WindowRegistry {}

impl WindowRegistry {
    pub fn handle(&self, role: WindowRole) -> Option<AnyWindowHandle> {
        self.handles.get(&role).copied()
    }

    pub fn set_handle(&mut self, role: WindowRole, handle: AnyWindowHandle) {
        self.handles.insert(role, handle);
    }

    pub fn forget(&mut self, role: WindowRole) {
        self.handles.remove(&role);
        self.sessions.closed(role);
    }
}

/// Single-instance open-or-focus for a window role. If the recorded handle is
/// stale (the window was closed some other way), the session model is
/// corrected and a fresh window opens.
pub fn open_or_focus(
    role: WindowRole,
    open_new: impl FnOnce(&mut App) -> AnyWindowHandle,
    cx: &mut App,
) {
    let decision = cx
        .global_mut::<WindowRegistry>()
        .sessions
        .request_open(role);
    if decision == OpenDecision::FocusExisting {
        let handle = cx.global::<WindowRegistry>().handle(role);
        if focus_window(&handle, cx) {
            return;
        }
        cx.global_mut::<WindowRegistry>().forget(role);
    }
    let handle = open_new(cx);
    cx.global_mut::<WindowRegistry>().set_handle(role, handle);
}

pub struct AppState {
    pub store: Store,
    environment: EnvironmentState,
    /// Handle to the module registry living inside the store's provider.
    module_provider: Arc<ModuleProvider>,
    /// Held for the process lifetime; the watcher thread is event-driven and
    /// does no idle polling.
    environment_watcher: Option<crate::environment_watch::EnvironmentWatcher>,
    /// Held for the process lifetime: debounced watchers over the read
    /// backends' config paths (dconf db, Alacritty config dir).
    settings_watcher: Option<crate::settings_watch::SettingsWatcher>,
    /// Bumped per background read pass; stale results must not publish.
    reads_generation: u64,
    /// True when the provider's router serves fixture paths (the `reads`
    /// demo): background reads then run against fixtures even though
    /// discovery is demo-locked.
    reads_fixture_backed: bool,
}

impl EventEmitter<AppEvent> for AppState {}

impl AppState {
    pub fn new(store: Store, module_provider: Arc<ModuleProvider>) -> Self {
        Self {
            store,
            environment: EnvironmentState::default(),
            module_provider,
            environment_watcher: None,
            settings_watcher: None,
            reads_generation: 0,
            reads_fixture_backed: false,
        }
    }

    /// Mark the provider's backend router as fixture-backed (demo mode).
    pub fn set_reads_fixture_backed(&mut self) {
        self.reads_fixture_backed = true;
    }

    /// The module-registry handle (module metadata, provenance, app
    /// summaries for the Applications window).
    pub fn module_provider(&self) -> &Arc<ModuleProvider> {
        &self.module_provider
    }

    // ---- Environment discovery (Phase 03) + module applicability (04) -----

    /// Current snapshot, when at least one discovery stage has completed.
    pub fn environment_snapshot(&self) -> Option<Arc<EnvironmentSnapshot>> {
        self.environment.snapshot.clone()
    }

    /// The derived app/module list (rebuilt only when a snapshot is ingested,
    /// never per render).
    pub fn app_list(&self) -> &[AppListEntry] {
        &self.environment.app_list
    }

    pub fn scan_state(&self) -> ScanState {
        self.environment.scan
    }

    /// Publish a snapshot to the environment cache and re-evaluate module
    /// applicability against it. The store reloads only when the set of
    /// available settings changed (staged edits and navigation survive
    /// otherwise).
    fn apply_environment(&mut self, snapshot: &Arc<EnvironmentSnapshot>, cx: &mut Context<Self>) {
        let modules = self.app_module_refs();
        self.environment.app_list = build_app_list(
            &snapshot.installed_apps,
            &snapshot.available_backends,
            &modules,
        );
        self.environment.snapshot = Some(snapshot.clone());
        let changed = self.module_provider.set_environment(Some(snapshot));
        if changed {
            licoman_core::log::info(
                "ui::modules",
                "module applicability changed; reloading settings",
                &[],
            );
            self.store.reload_all();
            cx.emit(AppEvent::ModulesChanged);
        }
        // Real-backend rows are (re)read off the UI thread after every
        // ingest — first reads and refreshes alike.
        self.schedule_background_reads(cx);
        self.ensure_settings_watcher(cx);
    }

    /// Schedule one background read pass over every Background-latency
    /// setting (files, GSettings, D-Bus). Reads run on the background
    /// executor; results publish one row at a time; a newer pass (generation
    /// bump) silently supersedes stale results. A hanging backend cannot
    /// stall this: backends bound their own I/O (§6.4).
    pub fn schedule_background_reads(&mut self, cx: &mut Context<Self>) {
        // Demo fixtures are only re-read when the router itself is
        // fixture-backed; live backends must never run under a demo.
        if self.environment.demo_locked && !self.reads_fixture_backed {
            return;
        }
        self.reads_generation += 1;
        let generation = self.reads_generation;
        let ids = self.store.background_read_ids();
        if ids.is_empty() {
            return;
        }
        licoman_core::log::info(
            "ui::reads",
            "background read pass scheduled",
            &[("settings", ids.len().to_string())],
        );
        let provider = self.module_provider.clone();
        cx.spawn(async move |this, cx| {
            for id in ids {
                let provider = provider.clone();
                let id_for_read = id.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        licoman_core::provider::Provider::read_detailed(&*provider, &id_for_read)
                    })
                    .await;
                let published = this.update(cx, |state, cx| {
                    if state.reads_generation != generation {
                        return; // a newer pass owns the rows now
                    }
                    if state.store.publish_read(&id, result) {
                        cx.emit(AppEvent::SettingsChanged);
                    }
                });
                if published.is_err() {
                    return; // the app is shutting down
                }
            }
        })
        .detach();
    }

    /// The settings watcher fired (debounced): re-read the backends' values.
    /// Demo fixtures are never replaced by live data.
    pub fn on_settings_changed_on_disk(&mut self, cx: &mut Context<Self>) {
        if self.environment.demo_locked {
            return;
        }
        licoman_core::log::info(
            "ui::reads",
            "watched configuration path changed; refreshing setting values",
            &[],
        );
        self.schedule_background_reads(cx);
    }

    /// Attach (or re-attach) the debounced watcher over the read backends'
    /// config paths. Called after discovery ingests; paths only change then.
    pub fn ensure_settings_watcher(&mut self, cx: &mut Context<Self>) {
        if self.environment.demo_locked {
            return;
        }
        let paths = self.module_provider.router().watch_paths();
        if paths.is_empty() {
            return;
        }
        if self
            .settings_watcher
            .as_ref()
            .is_some_and(|w| w.paths() == paths)
        {
            return; // unchanged; keep the existing watcher
        }
        if let Some(watcher) = crate::settings_watch::start(cx.weak_entity(), paths, cx) {
            self.settings_watcher = Some(watcher);
        }
    }

    /// Application-scope module summaries (cloned out of the registry under
    /// a short lock).
    fn app_module_refs(&self) -> Vec<licoman_core::modules::AppModuleRef> {
        self.module_provider
            .with_registry(|registry| registry.app_modules())
    }

    /// Start (or restart) discovery off the UI thread. Any in-flight scan is
    /// cancelled first; stages publish partial snapshots as they complete.
    pub fn start_discovery(&mut self, cx: &mut Context<Self>) {
        if self.environment.demo_locked {
            return;
        }
        self.environment.control.cancel();
        self.environment.generation += 1;
        let generation = self.environment.generation;
        let control = Arc::new(ScanControl::new());
        self.environment.control = control.clone();
        self.environment.scan = ScanState::Loading {
            stage: ScanStage::Distribution,
        };
        cx.emit(AppEvent::EnvironmentChanged);

        cx.spawn(async move |this, cx| {
            let sources = DiscoverySources::from_environment();
            let mut run = DiscoveryRun::new();
            while let Some(stage) = run.next_stage() {
                // The stage itself runs on the background executor; the UI
                // thread only publishes results. `run` travels into the
                // closure and back so the loop keeps owning it.
                let stage_control = control.clone();
                let sources = sources.clone();
                let status;
                (run, status) = cx
                    .background_executor()
                    .spawn(async move {
                        let status = run.run_next_stage(&sources, &stage_control, &|_| {});
                        (run, status)
                    })
                    .await;
                let finished = run.next_stage().is_none();
                let published = this.update(cx, |state, cx| {
                    if state.environment.generation != generation {
                        return; // a newer scan owns the cache now
                    }
                    let scan = match (status, finished) {
                        (ScanStatus::Cancelled, _) => ScanState::Cancelled,
                        (ScanStatus::Completed, true) => ScanState::Ready,
                        (ScanStatus::Completed, false) => ScanState::Loading { stage },
                    };
                    state.environment.ingest(&run, scan);
                    if let Some(snapshot) = state.environment.snapshot.clone() {
                        state.apply_environment(&snapshot, cx);
                    }
                    cx.emit(AppEvent::EnvironmentChanged);
                });
                if published.is_err() {
                    return; // the app is shutting down
                }
                if status == ScanStatus::Cancelled {
                    return;
                }
            }
        })
        .detach();
    }

    /// User-visible Refresh: cancels the in-flight scan and starts a new one.
    pub fn refresh_environment(&mut self, cx: &mut Context<Self>) {
        self.start_discovery(cx);
    }

    /// Filesystem watcher fired (debounced): re-scan, keeping the UI
    /// responsive. Demo fixtures are never replaced by live data.
    pub fn on_environment_changed_on_disk(&mut self, cx: &mut Context<Self>) {
        if self.environment.demo_locked {
            return;
        }
        licoman_core::log::info(
            "ui::environment",
            "application directories changed; refreshing discovery",
            &[],
        );
        self.start_discovery(cx);
    }

    /// Install the debounced application-directory watcher (once).
    pub fn ensure_environment_watcher(&mut self, cx: &mut Context<Self>) {
        if self.environment.demo_locked {
            return;
        }
        let dirs: Vec<std::path::PathBuf> = DiscoverySources::from_environment()
            .app_dirs
            .into_iter()
            .map(|dir| dir.path)
            .collect();
        if let Some(watcher) = crate::environment_watch::start(cx.weak_entity(), dirs, cx) {
            self.environment_watcher = Some(watcher);
        }
    }

    /// Replace discovery with a deterministic demo fixture (visual
    /// verification of environments this machine cannot run).
    pub fn load_demo_environment(&mut self, name: &str, cx: &mut Context<Self>) -> bool {
        let Some(snapshot) = licoman_core::discovery::demo_fixtures::demo_snapshot(name) else {
            return false;
        };
        self.environment.demo_locked = true;
        self.environment.scan = ScanState::Ready;
        let snapshot = Arc::new(snapshot);
        self.apply_environment(&snapshot, cx);
        cx.emit(AppEvent::EnvironmentChanged);
        true
    }

    /// Resolve the Applications-window selection identity for a module
    /// (search-palette navigation): the installed app's identity when the
    /// module matches one, else the module id.
    pub fn select_identity_for_module(&self, module_id: &str) -> Option<String> {
        self.environment
            .app_list
            .iter()
            .find(|entry| entry.module.as_deref() == Some(module_id))
            .map(|entry| entry.identity.clone())
            .or_else(|| Some(module_id.to_string()))
    }

    /// Forward a typed command to the core store and emit the matching
    /// events. This is the only path from UI intent to domain mutation.
    pub fn dispatch(&mut self, command: Command, cx: &mut Context<Self>) -> DispatchReport {
        let is_apply_all = matches!(command, Command::ApplyAll);
        let report = self.store.dispatch(command);
        if report.settings_changed {
            cx.emit(AppEvent::SettingsChanged);
        }
        if report.preferences_changed {
            cx.emit(AppEvent::PreferencesChanged);
        }

        // The staged apply keeps a visible interval so "Applying…" is
        // truthful, then completes on the main thread. The only timers in the
        // app run while an apply or its confirmation is in flight — there is
        // no background daemon and no idle polling.
        if is_apply_all && self.store.applying() {
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(700))
                    .await;
                let _ = this.update(cx, |state, cx| state.complete_apply(cx));
            })
            .detach();
        }
        report
    }

    /// Finish a staged apply and schedule the transient "Saved" confirmation.
    pub fn complete_apply(&mut self, cx: &mut Context<Self>) {
        let report = self.store.apply_complete();
        if report.settings_changed {
            cx.emit(AppEvent::SettingsChanged);
        }
        if self
            .store
            .models()
            .values()
            .any(|m| m.phase == SettingPhase::Applied)
        {
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(1800))
                    .await;
                let _ = this.update(cx, |state, cx| {
                    if state.store.acknowledge_applied() {
                        cx.emit(AppEvent::SettingsChanged);
                    }
                });
            })
            .detach();
        }
    }

    /// Change Licoman's own appearance preference (immediate write to
    /// Licoman's config through the core) and re-resolve the concrete theme
    /// for every window. Distinct from provider-backed setting edits.
    pub fn change_theme(&mut self, mode: AppearanceMode, cx: &mut Context<Self>) {
        self.dispatch(Command::SetAppearance(mode), cx);
        self.resolve_and_apply_theme(cx);
    }

    /// Resolve the selected appearance mode to a concrete gpui theme mode and
    /// apply it to every window. `System` resolves through the desktop
    /// appearance signal; an unavailable signal conservatively means Dark
    /// (disclosed in Preferences).
    pub fn resolve_and_apply_theme(&self, cx: &mut App) {
        let mode = match self.store.preferences().theme {
            AppearanceMode::Dark => gpui_kit::component::theme::ThemeMode::Dark,
            AppearanceMode::Light => gpui_kit::component::theme::ThemeMode::Light,
            AppearanceMode::System => system_theme_mode(cx.window_appearance()),
        };
        gpui_kit::component::theme::Theme::change(mode, None, cx);
    }

    /// Record one navigation step in a role's independent history.
    pub fn push_history(&self, role: WindowRole, entry: impl Into<String>, cx: &mut App) {
        cx.global_mut::<WindowRegistry>()
            .sessions
            .push_history(role, entry);
    }
}

/// Map gpui's desktop appearance signal to a concrete theme mode.
pub fn system_theme_mode(
    appearance: gpui_kit::WindowAppearance,
) -> gpui_kit::component::theme::ThemeMode {
    use gpui_kit::component::theme::ThemeMode;
    match appearance {
        gpui_kit::WindowAppearance::Dark | gpui_kit::WindowAppearance::VibrantDark => {
            ThemeMode::Dark
        }
        gpui_kit::WindowAppearance::Light | gpui_kit::WindowAppearance::VibrantLight => {
            ThemeMode::Light
        }
    }
}

/// Focus an existing window if present; returns whether it existed and was
/// still alive.
pub fn focus_window(handle: &Option<AnyWindowHandle>, cx: &mut App) -> bool {
    match handle {
        Some(h) => h
            .update(cx, |_view, window, _cx| {
                window.activate_window();
            })
            .is_ok(),
        None => false,
    }
}
