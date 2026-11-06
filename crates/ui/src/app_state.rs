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
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{AnyWindowHandle, App, Context, EventEmitter, Global};
use licoman_core::account::{AccountsClient as _, UserAccount, ZBusAccounts};
use licoman_core::command::DispatchReport;
use licoman_core::discovery::{
    AppListEntry, DiscoveryRun, DiscoverySources, EnvironmentSnapshot, ScanControl, ScanStage,
    ScanStatus, build_app_list,
};
use licoman_core::error::{ErrorKind, LicomanError};
use licoman_core::modules::ModuleProvider;
use licoman_core::prefs::AppearanceMode;
use licoman_core::read::dbus::ClientError;
use licoman_core::system::SystemFacts;
use licoman_core::window_sessions::WindowSessions;
use licoman_core::{Command, Scope, SettingPhase, Store, WindowRole};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppEvent {
    /// Setting descriptors, models or selections changed.
    SettingsChanged,
    /// Licoman preferences (theme) changed.
    PreferencesChanged,
    /// Environment discovery progressed (loading, partial or ready), or the
    /// Summary panel's machine facts were refreshed.
    EnvironmentChanged,
    /// Module applicability changed (a reload published new availability).
    ModulesChanged,
    /// Current-user identity, avatar or account-service status changed.
    AccountChanged,
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
    /// Machine facts for the Summary panel (collected off the UI thread,
    /// refreshed on Summary selection — uptime/memory go stale otherwise).
    system: Option<Arc<SystemFacts>>,
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
            system: None,
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

/// Which account change an operation refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountAction {
    FullName,
    Password,
    Picture,
}

/// The account panel's visible operation state.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum AccountOp {
    #[default]
    Idle,
    Busy(AccountAction),
    /// The last change succeeded (shown until the next action).
    Saved(AccountAction),
    /// The last change failed; the string is the user-safe explanation.
    Failed(AccountAction, String),
}

/// Everything the user card and account panel render, refreshed off the UI
/// thread. Reads work on any Linux system (passwd + ~/.face); only the
/// changes depend on the AccountsService daemon, whose availability is
/// probed honestly (`None` = still probing).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AccountState {
    pub info: Option<UserAccount>,
    /// First existing avatar candidate, when any.
    pub avatar: Option<PathBuf>,
    /// AccountsService availability: `None` = probing,
    /// `Some(false)` = daemon unreachable (view stays read-only).
    pub service: Option<bool>,
    pub op: AccountOp,
}

impl AccountState {
    /// True while an account change is in flight (buttons disable).
    pub fn op_busied(&self) -> bool {
        matches!(self.op, AccountOp::Busy(_))
    }
}

/// GPUI-side window registry: the main window's handle plus the navigation
/// history from the core [`WindowSessions`] model (unit tested headless).
/// Licoman is single-window: Applications and Settings render as content-area
/// panels, so there is no open-or-focus machinery anymore.
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

pub struct AppState {
    pub store: Store,
    environment: EnvironmentState,
    /// Current-user cache for the sidebar card and account panel.
    account: AccountState,
    /// Bumped per account read/op; stale results must not publish.
    account_generation: u64,
    /// Bumped per system-facts pass; stale collects must not publish.
    system_generation: u64,
    /// Handle to the module registry living inside the store's provider.
    module_provider: Arc<ModuleProvider>,
    /// Held for the process lifetime; the watcher thread is event-driven and
    /// does no idle polling.
    environment_watcher: Option<crate::environment_watch::EnvironmentWatcher>,
    /// Held for the process lifetime: debounced watchers over the read
    /// backends' config paths (dconf db, Alacritty config dir).
    settings_watcher: Option<crate::settings_watch::SettingsWatcher>,
    /// Held for the process lifetime: native GSettings change notifications
    /// (gio signals; 07 Integration B). None outside a live environment.
    gsettings_watcher: Option<crate::gsettings_watch::GSettingsWatcher>,
    xfconf_watcher: Option<crate::xfconf_watch::XfconfWatcher>,
    /// Bumped per background read pass; stale results must not publish.
    reads_generation: u64,
    /// Bumped per write pass; stale publishes must not land.
    apply_generation: u64,
    /// True when the provider's router serves fixture paths (the `reads`
    /// demo): background reads then run against fixtures even though
    /// discovery is demo-locked.
    reads_fixture_backed: bool,
    /// Read handles for providers outside the module registry (Phase 07
    /// diagnostics providers), each with the ids it owns. The read pass
    /// dispatches per id: module ids go to the module provider, these to
    /// their own handles.
    extra_readers: Vec<(
        std::collections::HashSet<String>,
        Arc<dyn licoman_core::Provider>,
    )>,
}

impl EventEmitter<AppEvent> for AppState {}

/// ClientError → user-safe [`LicomanError`] for account flows: the honest
/// daemon-absent explanation when applicable, the mapped kind otherwise.
fn account_licoman_error(error: &ClientError) -> LicomanError {
    LicomanError::new(
        ErrorKind::Unavailable,
        licoman_core::account::user_message(error),
        format!("account: {error}"),
    )
}

impl AppState {
    pub fn new(store: Store, module_provider: Arc<ModuleProvider>) -> Self {
        Self {
            store,
            environment: EnvironmentState::default(),
            account: AccountState::default(),
            account_generation: 0,
            system_generation: 0,
            module_provider,
            environment_watcher: None,
            settings_watcher: None,
            gsettings_watcher: None,
            xfconf_watcher: None,
            reads_generation: 0,
            apply_generation: 0,
            reads_fixture_backed: false,
            extra_readers: Vec::new(),
        }
    }

    /// Register a non-module read provider whose ids the background read
    /// pass must serve (e.g. the Hyprland diagnostics provider).
    pub fn add_extra_reader(&mut self, provider: Arc<dyn licoman_core::Provider>) {
        let ids: std::collections::HashSet<String> = provider
            .descriptors()
            .iter()
            .map(|descriptor| descriptor.id.clone())
            .collect();
        self.extra_readers.push((ids, provider));
    }

    /// The read handle that owns `id`: the module provider when the module
    /// registry declares it, else the registered extra reader.
    fn reader_for(&self, id: &str) -> Arc<dyn licoman_core::Provider> {
        let module_owned = self
            .module_provider
            .with_registry(|registry| registry.setting(id).is_some());
        if module_owned {
            return self.module_provider.clone();
        }
        for (ids, provider) in &self.extra_readers {
            if ids.contains(id) {
                return provider.clone();
            }
        }
        self.module_provider.clone()
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

    // ---- Summary facts + current account (read-only caches) --------------

    /// The machine facts behind the Summary panel, when collected.
    pub fn system_facts(&self) -> Option<&Arc<SystemFacts>> {
        self.environment.system.as_ref()
    }

    /// Collect the Summary panel's facts on the background executor.
    /// Triggered at startup and whenever the Summary scope is selected —
    /// uptime, memory and disk usage go stale otherwise.
    pub fn refresh_system_facts(&mut self, cx: &mut Context<Self>) {
        self.system_generation += 1;
        let generation = self.system_generation;
        cx.spawn(async move |this, cx| {
            let facts = cx
                .background_executor()
                .spawn(async { SystemFacts::collect() })
                .await;
            let _ = this.update(cx, |state, cx| {
                if state.system_generation != generation {
                    return; // a newer pass owns the facts now
                }
                state.environment.system = Some(Arc::new(facts));
                cx.emit(AppEvent::EnvironmentChanged);
            });
        })
        .detach();
    }

    /// The account cache the sidebar card and the account panel render.
    pub fn account(&self) -> &AccountState {
        &self.account
    }

    /// Re-read the current user, avatar and AccountsService availability on
    /// the background executor. Runs at startup and after every change.
    pub fn refresh_account(&mut self, cx: &mut Context<Self>) {
        self.account_generation += 1;
        let generation = self.account_generation;
        cx.spawn(async move |this, cx| {
            let detected = cx
                .background_executor()
                .spawn(async {
                    let info = UserAccount::detect();
                    let avatar = info.as_ref().and_then(UserAccount::first_avatar);
                    (info, avatar)
                })
                .await;
            // Passive probe: a FindUserById round-trip decides availability.
            // No daemon is started; absence is an honest Some(false).
            let service = detected
                .0
                .as_ref()
                .map(|info| ZBusAccounts::default().find_user_path(info.uid).is_ok());
            let _ = this.update(cx, |state, cx| {
                if state.account_generation != generation {
                    return; // a newer read/op owns the account now
                }
                let op = state.account.op.clone();
                state.account = AccountState {
                    info: detected.0,
                    avatar: detected.1,
                    service,
                    op,
                };
                cx.emit(AppEvent::AccountChanged);
            });
        })
        .detach();
    }

    /// Change the current user's full name through AccountsService.
    pub fn set_account_full_name(&mut self, name: String, cx: &mut Context<Self>) {
        self.run_account_change(
            AccountAction::FullName,
            move |client, path| {
                client
                    .set_real_name(path, name.trim())
                    .map_err(|error| account_licoman_error(&error))
            },
            cx,
        );
    }

    /// Change the current user's password through AccountsService
    /// (polkit's change-own-user-data; the GUI never collects a password
    /// for authorization).
    pub fn set_account_password(&mut self, password: String, cx: &mut Context<Self>) {
        self.run_account_change(
            AccountAction::Password,
            move |client, path| {
                client
                    .set_password(path, &password)
                    .map_err(|error| account_licoman_error(&error))
            },
            cx,
        );
    }

    /// Set the account picture: copy the picked image to the `~/.face`
    /// convention (durable, works without the daemon), then point
    /// AccountsService at it (best-effort — absent daemon is fine).
    pub fn set_account_avatar(&mut self, picked: PathBuf, cx: &mut Context<Self>) {
        self.run_account_change(
            AccountAction::Picture,
            move |client, path| {
                let face = picked.clone();
                if let Some(info) = UserAccount::detect() {
                    let target = info.home.join(".face");
                    std::fs::copy(&face, &target).map_err(|error| {
                        LicomanError::new(
                            ErrorKind::Io,
                            "The picture could not be copied to ~/.face; \
                             nothing was changed.",
                            format!("fs::copy: {error}"),
                        )
                    })?;
                    let _ = client.set_icon_file(path, &target.to_string_lossy());
                }
                Ok(())
            },
            cx,
        );
    }

    /// Remove the account picture: delete the `~/.face` convention files
    /// and clear AccountsService's icon (best-effort — the local removal
    /// works even when the daemon is absent, since the files are home-owned).
    pub fn remove_account_avatar(&mut self, cx: &mut Context<Self>) {
        self.account.op = AccountOp::Busy(AccountAction::Picture);
        cx.emit(AppEvent::AccountChanged);
        self.account_generation += 1;
        let generation = self.account_generation;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let Some(info) = UserAccount::detect() else {
                        return Err(LicomanError::new(
                            ErrorKind::Unavailable,
                            "The current user could not be identified, so no \
                             account change was attempted.",
                            "account: user detect failed",
                        ));
                    };
                    for name in [".face", ".face.icon"] {
                        let target = info.home.join(name);
                        if let Err(error) = std::fs::remove_file(&target)
                            && error.kind() != std::io::ErrorKind::NotFound
                        {
                            return Err(LicomanError::new(
                                ErrorKind::Io,
                                "The picture file could not be removed; nothing \
                                 else was changed.",
                                format!("remove_file: {error}"),
                            ));
                        }
                    }
                    // Best-effort daemon clear (absent daemon is fine —
                    // ~/.face is already gone and display reads it first).
                    let client = ZBusAccounts::default();
                    if let Ok(user_path) = client.find_user_path(info.uid) {
                        let _ = client.set_icon_file(&user_path, "");
                    }
                    Ok(())
                })
                .await;
            let _ = this.update(cx, |state, cx| {
                state.publish_account_result(AccountAction::Picture, generation, result, cx)
            });
        })
        .detach();
    }

    /// Record a picture-pick failure that happened outside the change flow
    /// (the portal round-trip failed before any copy/set was attempted).
    pub fn fail_account_picture(&mut self, message: String, cx: &mut Context<Self>) {
        self.account.op = AccountOp::Failed(AccountAction::Picture, message);
        cx.emit(AppEvent::AccountChanged);
    }

    /// Publish one finished account operation: Saved + re-read, or Failed
    /// with the error's user-safe message. Stale generations are dropped.
    fn publish_account_result(
        &mut self,
        action: AccountAction,
        generation: u64,
        result: Result<(), LicomanError>,
        cx: &mut Context<Self>,
    ) {
        if self.account_generation != generation {
            return; // a newer op owns the account now
        }
        match result {
            Ok(()) => {
                self.account.op = AccountOp::Saved(action);
                cx.emit(AppEvent::AccountChanged);
                self.refresh_account(cx);
            }
            Err(error) => {
                self.account.op = AccountOp::Failed(action, error.user_message.clone());
                cx.emit(AppEvent::AccountChanged);
            }
        }
    }

    /// Shared account-change flow: show Busy, locate the own-user object and
    /// run the typed change on the background executor, then publish
    /// Saved/Failed with a user-safe message and re-read the account.
    fn run_account_change<F>(&mut self, action: AccountAction, change: F, cx: &mut Context<Self>)
    where
        F: FnOnce(&ZBusAccounts, &str) -> Result<(), LicomanError> + Send + 'static,
    {
        self.account.op = AccountOp::Busy(action);
        cx.emit(AppEvent::AccountChanged);
        self.account_generation += 1;
        let generation = self.account_generation;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = ZBusAccounts::default();
                    let Some(info) = UserAccount::detect() else {
                        return Err(LicomanError::new(
                            ErrorKind::Unavailable,
                            "The current user could not be identified, so no \
                             account change was attempted.",
                            "account: user detect failed",
                        ));
                    };
                    let path = client
                        .find_user_path(info.uid)
                        .map_err(|error| account_licoman_error(&error))?;
                    change(&client, &path)
                })
                .await;
            let _ = this.update(cx, |state, cx| {
                state.publish_account_result(action, generation, result, cx)
            });
        })
        .detach();
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
        self.ensure_gsettings_watcher(cx);
        self.ensure_xfconf_watcher(cx);
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
        // Resolve each id's owning read handle up front (UI thread, registry
        // lookups only) so the background pass sends every read to the
        // provider that actually serves it.
        let jobs: Vec<(String, Arc<dyn licoman_core::Provider>)> = ids
            .into_iter()
            .map(|id| {
                let handle = self.reader_for(&id);
                (id, handle)
            })
            .collect();
        cx.spawn(async move |this, cx| {
            for (id, provider) in jobs {
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
                    // Failed background reads are honest unavailable rows;
                    // log them (id and error kind only — never values) so a
                    // live run's silent failures stay diagnosable.
                    if let Err(error) = &result {
                        licoman_core::log::info(
                            "ui::reads",
                            "background read failed",
                            &[
                                ("id", id.to_string()),
                                ("kind", format!("{:?}", error.kind)),
                            ],
                        );
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

    /// Attach (or re-attach) the native GSettings change watcher over the
    /// currently-applicable schema/key pairs (07 Integration B). Notifications
    /// run the same debounced refresh path as the filesystem watcher; a
    /// pending edit is never dropped — the refresh re-reads the source, and
    /// the apply-time conflict checks catch the moved value.
    pub fn ensure_gsettings_watcher(&mut self, cx: &mut Context<Self>) {
        if self.environment.demo_locked {
            return;
        }
        let keys = self.module_provider.gsettings_watch_keys();
        if keys.is_empty() {
            self.gsettings_watcher = None;
            return;
        }
        if self
            .gsettings_watcher
            .as_ref()
            .is_some_and(|w| w.keys() == keys.as_slice())
        {
            return; // unchanged; keep the existing watcher
        }
        if let Some(watcher) = crate::gsettings_watch::start(cx.weak_entity(), keys, cx) {
            self.gsettings_watcher = Some(watcher);
        }
    }

    /// Attach (or drop) the Xfconf signal watcher based on whether any
    /// applicable setting is xfconf-backed (Phase 08). The subscription is
    /// interface-wide; the debounced refresh re-reads only the applicable
    /// settings.
    pub fn ensure_xfconf_watcher(&mut self, cx: &mut Context<Self>) {
        if self.environment.demo_locked {
            return;
        }
        if !self.module_provider.has_xfconf_settings() {
            self.xfconf_watcher = None;
            return;
        }
        if self.xfconf_watcher.is_some() {
            return; // interface-wide subscription needs no per-key restart
        }
        if let Some(watcher) = crate::xfconf_watch::start(cx.weak_entity(), cx) {
            self.xfconf_watcher = Some(watcher);
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
        // Opening the Summary panel refreshes its volatile facts (uptime,
        // memory, disk usage) — a background read, not a mutation.
        if matches!(command, Command::SelectScope(Scope::Summary)) {
            self.refresh_system_facts(cx);
        }
        let report = self.store.dispatch(command);
        if report.settings_changed {
            cx.emit(AppEvent::SettingsChanged);
        }
        if report.preferences_changed {
            cx.emit(AppEvent::PreferencesChanged);
        }

        // Any command can leave rows in the Applying phase (staged Apply,
        // immediate edit, retry, undo, reset, confirm). The visible
        // "Applying…" interval runs first, then the write pass executes the
        // rows' intents on the background executor — never on the UI thread.
        if self.store.applying() {
            self.kick_apply(cx);
        }
        report
    }

    /// Start the visible apply interval; the write pass follows it. The
    /// generation guard makes repeated clicks and overlapping commands safe:
    /// a newer pass silently supersedes an older one.
    fn kick_apply(&mut self, cx: &mut Context<Self>) {
        self.apply_generation += 1;
        let generation = self.apply_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            let _ = this.update(cx, |state, cx| state.run_apply_pass(generation, cx));
        })
        .detach();
    }

    /// Drain the store's staged intents and execute them on the background
    /// executor, publishing each result as it lands. Writes through the safe
    /// write engine are conflict-checked, backed up and verified by the
    /// engine itself; the store's [`Store::publish_apply`] maps the outcomes
    /// (no "Applied" without a verification that matched).
    fn run_apply_pass(&mut self, generation: u64, cx: &mut Context<Self>) {
        if self.apply_generation != generation {
            return; // a newer pass owns the rows now
        }
        let intents = self.store.take_apply_intents();
        if intents.is_empty() {
            return;
        }
        licoman_core::log::info(
            "ui::writes",
            "background write pass scheduled",
            &[("changes", intents.len().to_string())],
        );
        let provider = self.module_provider.clone();
        cx.spawn(async move |this, cx| {
            for (id, intent) in intents {
                let provider = provider.clone();
                let intent_for_task = intent.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        licoman_core::provider::Provider::write_intent(&*provider, &intent_for_task)
                    })
                    .await;
                let published = this.update(cx, |state, cx| {
                    if state.apply_generation != generation {
                        return; // a newer pass owns the rows now
                    }
                    if state.store.publish_apply(&id, &intent, result) {
                        cx.emit(AppEvent::SettingsChanged);
                    }
                    state.schedule_acknowledgement(cx);
                });
                if published.is_err() {
                    return; // the app is shutting down
                }
            }
        })
        .detach();
    }

    /// Schedule the transient "Saved" confirmation once applied rows exist.
    fn schedule_acknowledgement(&mut self, cx: &mut Context<Self>) {
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
