//! Shared application state: Licoman preferences, mock setting model, window
//! registry and the apply lifecycle. This is the single source of truth both
//! native windows observe (01_DESIGN_SYSTEM.md §4: shared theme and setting
//! state, independent per-window navigation).

use std::time::Duration;

use gpui_kit::{AnyWindowHandle, App, Context, EventEmitter, Global};

use crate::mock::{ControlKind, MOCK_ENVIRONMENT, MockSetting, MockValue, RowStatus, Scope};
use crate::prefs::{AppearanceMode, LoadDiagnostic, Preferences};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppEvent {
    /// A mock setting changed value or state.
    SettingsChanged,
    /// Licoman preferences (theme) changed.
    PreferencesChanged,
}

/// Window registry so nav actions can open-or-focus windows.
#[derive(Default)]
pub struct WindowRegistry {
    pub apps_window: Option<AnyWindowHandle>,
    pub preferences_window: Option<AnyWindowHandle>,
}

impl Global for WindowRegistry {}

pub struct AppState {
    pub preferences: Preferences,
    pub preference_diagnostics: Vec<LoadDiagnostic>,
    pub settings: Vec<MockSetting>,
    pub selected_scope: Scope,
    pub selected_app: Option<usize>,
    pub applying: bool,
}

impl EventEmitter<AppEvent> for AppState {}

impl AppState {
    pub fn new(preferences: Preferences, preference_diagnostics: Vec<LoadDiagnostic>) -> Self {
        Self {
            preferences,
            preference_diagnostics,
            settings: crate::mock::mock_settings(),
            selected_scope: Scope::Shared,
            selected_app: Some(0),
            applying: false,
        }
    }

    pub fn set_theme_mode(&mut self, mode: AppearanceMode, cx: &mut Context<Self>) {
        if self.preferences.theme == mode {
            return;
        }
        self.preferences.theme = mode;
        if let Err(e) = crate::prefs::save(&self.preferences) {
            // Never let a preference write failure break the session; the
            // in-memory choice still applies for this run.
            eprintln!("licoman: could not save preferences: {e}");
        }
        self.resolve_and_apply_theme(cx);
        cx.emit(AppEvent::PreferencesChanged);
    }

    /// Resolve the selected appearance mode to a concrete gpui ThemeMode and
    /// apply it to every window. `System` resolves through the desktop
    /// appearance signal; an unavailable signal conservatively means Dark
    /// (disclosed in Preferences).
    pub fn resolve_and_apply_theme(&self, cx: &mut App) {
        let mode = match self.preferences.theme {
            AppearanceMode::Dark => gpui_kit::component::theme::ThemeMode::Dark,
            AppearanceMode::Light => gpui_kit::component::theme::ThemeMode::Light,
            AppearanceMode::System => system_theme_mode(cx.window_appearance()),
        };
        gpui_kit::component::theme::Theme::change(mode, None, cx);
    }

    pub fn dirty_count(&self) -> usize {
        self.settings.iter().filter(|s| s.is_dirty()).count()
    }

    pub fn setting(&self, id: &str) -> Option<&MockSetting> {
        self.settings.iter().find(|s| s.id.as_ref() == id)
    }

    fn setting_mut(&mut self, id: &str) -> Option<&mut MockSetting> {
        self.settings.iter_mut().find(|s| s.id.as_ref() == id)
    }

    /// Record a UI edit. A control value changing is **not** a persisted
    /// setting (02_FOUNDATION.md §4); it only marks the row dirty.
    pub fn edit(&mut self, id: &str, value: MockValue, cx: &mut Context<Self>) {
        let Some(setting) = self.setting_mut(id) else {
            return;
        };
        if setting.status.blocks_editing() || setting.kind == ControlKind::ReadOnly {
            return;
        }
        if setting.value == value {
            return;
        }
        setting.value = value;
        if !matches!(setting.status, RowStatus::Failed { .. }) {
            setting.status = RowStatus::Dirty;
        }
        cx.emit(AppEvent::SettingsChanged);
    }

    pub fn discard_all(&mut self, cx: &mut Context<Self>) {
        // Mock revert: dirty rows return to idle; this phase holds no baseline
        // snapshot because every value is showcase data.
        for setting in &mut self.settings {
            if setting.is_dirty() {
                setting.status = RowStatus::Idle;
            }
        }
        cx.emit(AppEvent::SettingsChanged);
    }

    /// Apply all pending changes through the mock lifecycle:
    /// Applying → (Saved | Failed) → Idle. No real configuration is touched.
    pub fn apply_pending(&mut self, cx: &mut Context<Self>) {
        if self.applying || self.dirty_count() == 0 {
            return;
        }
        self.applying = true;
        for setting in &mut self.settings {
            if setting.is_dirty() {
                setting.status = RowStatus::Applying;
            }
        }
        cx.emit(AppEvent::SettingsChanged);

        let failing: Vec<String> = self
            .settings
            .iter()
            .filter(|s| s.simulate_failure && s.is_dirty())
            .map(|s| s.id.to_string())
            .collect();

        cx.spawn(async move |this, cx| {
            // Simulated write latency; deterministic, no background daemon.
            cx.background_executor()
                .timer(Duration::from_millis(700))
                .await;
            let any_saved = this
                .update(cx, |state, cx| {
                    state.applying = false;
                    let mut any_saved = false;
                    for setting in &mut state.settings {
                        if matches!(setting.status, RowStatus::Applying) {
                            if failing.iter().any(|id| *id == setting.id) {
                                setting.status = RowStatus::Failed {
                                    reason: "The mock provider rejected this write.".into(),
                                };
                            } else {
                                setting.status = RowStatus::Saved;
                                any_saved = true;
                            }
                        }
                    }
                    cx.emit(AppEvent::SettingsChanged);
                    any_saved
                })
                .unwrap_or(false);

            // "Saved" is a transient confirmation, not a permanent label.
            cx.background_executor()
                .timer(Duration::from_millis(1800))
                .await;
            let _ = this.update(cx, |state, cx| {
                let mut changed = false;
                for setting in &mut state.settings {
                    if matches!(setting.status, RowStatus::Saved) {
                        setting.status = RowStatus::Idle;
                        changed = true;
                    }
                }
                if changed || any_saved {
                    cx.emit(AppEvent::SettingsChanged);
                }
            });
        })
        .detach();
    }

    pub fn retry_failed(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(setting) = self.setting_mut(id)
            && matches!(setting.status, RowStatus::Failed { .. })
        {
            setting.status = RowStatus::Dirty;
        }
        self.apply_pending(cx);
    }

    pub fn revert_failed(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(setting) = self.setting_mut(id)
            && matches!(setting.status, RowStatus::Failed { .. })
        {
            setting.status = RowStatus::Idle;
            cx.emit(AppEvent::SettingsChanged);
        }
    }

    pub fn acknowledge_external_change(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(setting) = self.setting_mut(id)
            && matches!(setting.status, RowStatus::ExternalChange)
        {
            setting.status = RowStatus::Idle;
            cx.emit(AppEvent::SettingsChanged);
        }
    }

    /// Search across all mock settings; returns matching indexes.
    pub fn search(&self, query: &str) -> Vec<usize> {
        let q = query.to_lowercase();
        if q.trim().is_empty() {
            return Vec::new();
        }
        self.settings
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                crate::mock::breadcrumb(s).to_lowercase().contains(&q)
                    || s.label.to_lowercase().contains(&q)
                    || s.description.to_lowercase().contains(&q)
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub fn environment(&self) -> &'static [(&'static str, &'static str)] {
        MOCK_ENVIRONMENT
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

/// Focus an existing window if present; returns whether it existed.
pub fn focus_window(handle: &Option<AnyWindowHandle>, cx: &mut App) -> bool {
    match handle {
        Some(h) => {
            let _ = h.update(cx, |_view, window, _cx| {
                window.activate_window();
            });
            true
        }
        None => false,
    }
}
