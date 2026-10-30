//! The view adapter: owns the core [`Store`] inside a GPUI entity, translates
//! UI intent into typed commands, emits observable events, drives the visible
//! apply interval, applies Licoman themes to every window, and models
//! single-instance window behavior with independent navigation histories.

use std::collections::HashMap;
use std::time::Duration;

use gpui_kit::{AnyWindowHandle, App, Context, EventEmitter, Global};
use licoman_core::command::DispatchReport;
use licoman_core::prefs::AppearanceMode;
use licoman_core::window_sessions::{OpenDecision, WindowSessions};
use licoman_core::{Command, SettingPhase, Store, WindowRole};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppEvent {
    /// Setting descriptors, models or selections changed.
    SettingsChanged,
    /// Licoman preferences (theme) changed.
    PreferencesChanged,
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
}

impl EventEmitter<AppEvent> for AppState {}

impl AppState {
    pub fn new(store: Store) -> Self {
        Self { store }
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
