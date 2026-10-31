//! Main window: scope sidebar, settings content, search palette, apply bar.
//! Structure follows 01_DESIGN_SYSTEM.md §4A. Since Phase 04 the content is
//! **registry-driven**: every row and control comes from validated module
//! manifests via the shared [`ControlSet`] factory — no setting id is
//! hard-coded here. Rendering reads owned snapshots from the core store;
//! every intent goes through `AppState::dispatch` with a typed command.

use crate::app_state::ScanState;
use gpui_kit::component::TitleBar;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::sidebar::{Sidebar, SidebarMenu, SidebarMenuItem};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::component::{Icon, IconName, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext, ClickEvent, Context, Entity, FontWeight, MouseButton, MouseDownEvent,
    ParentElement as _, Render, SharedString, Styled, Subscription, Window, div, px,
};
use gpui_kit::{
    Focusable as _, InteractiveElement as _, IntoElement as _, StatefulInteractiveElement as _,
};
use licoman_core::descriptor::{Scope, SettingDescriptor};
use licoman_core::error::ErrorKind;
use licoman_core::state::SettingPhase;
use licoman_core::{Command, SettingModel, WindowRole};

use crate::actions::*;
use crate::app_state::{AppEvent, AppState, open_or_focus};
use crate::theme::metrics;
use crate::ui::setting_controls::ControlSet;
use crate::ui::{OptionIntoAny as _, read_only_value, setting_row, settings_group, window_heading};
use licoman_core::prefs::AppearanceMode;

/// Owned render snapshot for one setting row.
#[derive(Clone)]
struct Row {
    descriptor: SettingDescriptor,
    model: SettingModel,
}

pub struct MainWindow {
    app: Entity<AppState>,
    header_search: Entity<InputState>,
    palette_input: Entity<InputState>,
    palette_open: bool,
    palette_selected: usize,
    /// Registry-driven controls for every editable setting in the sidebar
    /// scopes, keyed by stable setting id.
    controls: ControlSet,
    /// Set when the store changed; applied at the next render, where window
    /// access is available for control re-sync.
    resync_needed: bool,
    /// Set when module applicability changed; the control set must be
    /// rebuilt (rows that were unavailable can become editable).
    rebuild_needed: bool,
    /// Set when the header search submits; applied at the next render, where
    /// window access is available.
    pending_palette_query: Option<String>,
    _subscriptions: Vec<Subscription>,
    _appearance_sub: Option<Subscription>,
}

impl MainWindow {
    pub fn new(app: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let header_search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search all settings… (Ctrl+K)"));
        let palette_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search all settings…"));

        // One control set over every row in the sidebar scopes (Phase 04:
        // the factory derives controls from descriptors, never from ids).
        let mut all_rows: Vec<(SettingDescriptor, SettingModel)> = Vec::new();
        for scope in Scope::SIDEBAR {
            all_rows.extend(app.read(cx).store.scope_rows(scope));
        }
        let controls = ControlSet::build(&app, &all_rows, window, cx);

        let mut subscriptions = Vec::new();

        // The header search is the current-window entry into the global
        // search: pressing Enter opens the global palette with the query.
        // Window work happens in render (subscriptions have no window).
        subscriptions.push(cx.subscribe(&header_search, {
            move |this, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.pending_palette_query = Some(input.read(cx).value().to_string());
                    cx.notify();
                }
            }
        }));

        // Track app events: resync owned controls when the store changes
        // (discard, apply, module reload) so controls never present a stale
        // value.
        subscriptions.push(cx.subscribe(&app, {
            move |this, _app, event: &AppEvent, cx| match event {
                AppEvent::SettingsChanged => {
                    this.resync_needed = true;
                    cx.notify();
                }
                AppEvent::ModulesChanged => {
                    this.rebuild_needed = true;
                    this.resync_needed = false;
                    cx.notify();
                }
                AppEvent::PreferencesChanged | AppEvent::EnvironmentChanged => cx.notify(),
            }
        }));

        // Palette Enter accepts the highlighted result.
        subscriptions.push(cx.subscribe(&palette_input, {
            move |this, _input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.accept_palette_result(cx);
                }
            }
        }));

        // Follow the desktop light/dark signal while `System` is selected.
        // The subscription must live as long as the window.
        let appearance_app = app.clone();
        let appearance_sub = window.observe_window_appearance(move |_window, cx| {
            appearance_app.update(cx, |state, cx| {
                if state.store.preferences().theme == AppearanceMode::System {
                    state.resolve_and_apply_theme(cx);
                }
            });
        });

        let mut this = Self {
            app,
            header_search,
            palette_input,
            palette_open: false,
            palette_selected: 0,
            controls,
            resync_needed: false,
            rebuild_needed: false,
            pending_palette_query: None,
            _subscriptions: subscriptions,
            _appearance_sub: Some(appearance_sub),
        };
        this.sync_owned_controls(window, cx);
        this
    }

    /// Push store values into the controls that keep local display state
    /// (Select, Slider, Input) so the UI never shows a stale value after
    /// discard/apply/reload.
    fn sync_owned_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.resync_needed = false;
        let mut all_rows: Vec<(SettingDescriptor, SettingModel)> = Vec::new();
        for scope in Scope::SIDEBAR {
            all_rows.extend(self.app.read(cx).store.scope_rows(scope));
        }
        self.controls.sync(&all_rows, window, cx);
    }

    /// Rebuild the control set after module applicability changed (rows
    /// that were unavailable at build time can now be editable).
    fn rebuild_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rebuild_needed = false;
        let mut all_rows: Vec<(SettingDescriptor, SettingModel)> = Vec::new();
        for scope in Scope::SIDEBAR {
            all_rows.extend(self.app.read(cx).store.scope_rows(scope));
        }
        self.controls.rebuild(&self.app, &all_rows, window, cx);
    }

    /// Visual-verification hook (`LICOMAN_DEMO=palette`): opens the palette
    /// without pointer/keyboard interaction.
    pub fn demo_open_palette(&mut self, cx: &mut Context<Self>) {
        self.palette_open = true;
        self.palette_selected = 0;
        cx.notify();
    }

    fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = true;
        self.palette_selected = 0;
        let handle = self.palette_input.read(cx).focus_handle(cx).clone();
        window.focus(&handle, cx);
        cx.notify();
    }

    fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = false;
        // Return focus to the header search field (the invoking control).
        let handle = self.header_search.read(cx).focus_handle(cx).clone();
        window.focus(&handle, cx);
        cx.notify();
    }

    fn palette_results(&self, cx: &Context<Self>) -> Vec<licoman_core::search::SearchHit> {
        let query = self.palette_input.read(cx).value().to_string();
        self.app.read(cx).store.search(&query)
    }

    fn accept_palette_result(&mut self, cx: &mut Context<Self>) {
        let results = self.palette_results(cx);
        let picked = results
            .get(self.palette_selected.min(results.len().saturating_sub(1)))
            .cloned();
        if let Some(hit) = picked {
            if hit.scope == Scope::Application {
                // Application settings navigate into the Applications window
                // (00_START_HERE.md IA); selection is by stable identity.
                let module_id = hit.module_id.clone();
                self.app.update(cx, |state, cx| {
                    let identity = state.select_identity_for_module(&module_id);
                    state.dispatch(Command::SelectApp { id: identity }, cx);
                });
                self.open_applications_window(cx);
            } else {
                self.app.update(cx, |state, cx| {
                    state.dispatch(Command::SelectScope(hit.scope), cx);
                    state.push_history(
                        WindowRole::Main,
                        format!("scope:{}", hit.scope.label()),
                        cx,
                    );
                });
            }
        }
        self.palette_open = false;
        cx.notify();
    }

    // ---- row rendering ----------------------------------------------------

    fn render_setting(
        &mut self,
        index: usize,
        row: &Row,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let control = self.render_control(row, window, cx);
        let status = self.render_status(row, cx);
        setting_row(
            SharedString::from(format!("setting-row-{index}")),
            row.descriptor.label.clone(),
            row.descriptor.description.clone(),
            control,
            status,
            &theme,
        )
        .into_any_element()
    }

    fn render_control(
        &mut self,
        row: &Row,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let unavailable_display = match &row.model.error {
            Some(error) if error.kind == ErrorKind::Permission => "Not available",
            _ => "Unavailable",
        };

        // Unavailable rows (cannot be read here) never show a fake value.
        if row.model.phase == SettingPhase::Unavailable {
            return read_only_value(unavailable_display, &theme).into_any_element();
        }

        self.controls
            .render(&row.descriptor, &row.model, &self.app, &theme)
    }

    fn render_status(&mut self, row: &Row, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme().clone();
        let id = row.descriptor.id.clone();
        match &row.model.phase {
            SettingPhase::Unknown => Some(
                div()
                    .text_color(theme.muted_foreground)
                    .child("Not read yet"),
            )
            .into_any(),
            SettingPhase::Unavailable => {
                let error = row.model.error.as_ref()?;
                if error.kind == ErrorKind::Permission {
                    let authorize_id = SharedString::from(format!("authorize-{id}"));
                    Some(
                        v_flex()
                            .gap_1()
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .text_color(theme.muted_foreground)
                                    .child(
                                        Icon::new(gpui_kit::assets::IconName::Lock).small(),
                                    )
                                    .child(error.user_message.clone()),
                            )
                            .child(
                                h_flex().gap_2().child(
                                    Button::new(authorize_id)
                                        .label("Authorize…")
                                        .outline()
                                        .xsmall()
                                        .on_click(cx.listener(
                                            |_this, _: &ClickEvent, window, cx| {
                                                window.open_alert_dialog(cx, |alert, _window, _cx| {
                                                    alert
                                                        .title(
                                                            "Authorization is not available in this build",
                                                        )
                                                        .description(
                                                            "Licoman runs unprivileged. Privileged edits are deferred until the safe write engine and its authorization flow are implemented and reviewed (Phases 06–07). No system configuration was modified.",
                                                        )
                                                });
                                            },
                                        )),
                                ),
                            ),
                    )
                    .into_any()
                } else {
                    Some(
                        div()
                            .text_color(theme.muted_foreground)
                            .child(error.user_message.clone()),
                    )
                    .into_any()
                }
            }
            SettingPhase::Idle => {
                // Static provider notes (why a row is read-only) and known
                // restart requirements are truthful status lines.
                if let Some(note) = &row.descriptor.note {
                    return Some(div().text_color(theme.muted_foreground).child(note.clone()))
                        .into_any();
                }
                if let Some(target) = row
                    .descriptor
                    .restart_target
                    .clone()
                    .or_else(|| model_restart_target(&row.model))
                {
                    return Some(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .text_color(theme.warning)
                            .child(Icon::new(IconName::TriangleAlert).small())
                            .child(format!("Applies after restarting {target}")),
                    )
                    .into_any();
                }
                None
            }
            SettingPhase::Dirty => Some(
                h_flex()
                    .gap_1()
                    .items_center()
                    .text_color(theme.warning)
                    .child(Icon::new(IconName::CircleAlert).small())
                    .child("Unsaved change"),
            )
            .into_any(),
            SettingPhase::Validating => {
                Some(div().text_color(theme.muted_foreground).child("Checking…")).into_any()
            }
            SettingPhase::Pending => Some(
                div()
                    .text_color(theme.muted_foreground)
                    .child("Ready to apply"),
            )
            .into_any(),
            SettingPhase::Applying => Some(
                h_flex()
                    .gap_1()
                    .items_center()
                    .text_color(theme.muted_foreground)
                    .child("Applying…"),
            )
            .into_any(),
            SettingPhase::Applied => Some(
                h_flex()
                    .gap_1()
                    .items_center()
                    .text_color(theme.success)
                    .child(Icon::new(IconName::CircleCheck).small())
                    .child(if row.model.restart_required {
                        format!(
                            "Saved — applies after restarting {}",
                            row.descriptor
                                .restart_target
                                .clone()
                                .unwrap_or_else(|| "the affected program".to_string())
                        )
                    } else {
                        "Saved".to_string()
                    }),
            )
            .into_any(),
            SettingPhase::Failed => {
                let error = row.model.error.as_ref()?;
                let retry_id = SharedString::from(format!("retry-{id}"));
                let revert_id = SharedString::from(format!("revert-{id}"));
                let retry_setting_id = id.clone();
                let revert_setting_id = id.clone();
                Some(
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_1()
                                .items_center()
                                .text_color(theme.danger)
                                .child(Icon::new(IconName::CircleX).small())
                                .child(format!("Failed — {}", error.user_message)),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Button::new(retry_id)
                                        .label("Retry")
                                        .outline()
                                        .xsmall()
                                        .on_click(cx.listener(
                                            move |this, _: &ClickEvent, _window, cx| {
                                                let setting_id = retry_setting_id.clone();
                                                this.app.update(cx, |state, cx| {
                                                    state.dispatch(
                                                        Command::Retry { id: setting_id },
                                                        cx,
                                                    );
                                                });
                                            },
                                        )),
                                )
                                .child(
                                    Button::new(revert_id)
                                        .label("Revert")
                                        .ghost()
                                        .xsmall()
                                        .on_click(cx.listener(
                                            move |this, _: &ClickEvent, _window, cx| {
                                                let setting_id = revert_setting_id.clone();
                                                this.app.update(cx, |state, cx| {
                                                    state.dispatch(
                                                        Command::Revert { id: setting_id },
                                                        cx,
                                                    );
                                                });
                                            },
                                        )),
                                ),
                        ),
                )
                .into_any()
            }
            SettingPhase::ExternalConflict => {
                let reload_id = SharedString::from(format!("reload-{id}"));
                let compare_id = SharedString::from(format!("compare-{id}"));
                let reload_setting_id = id.clone();
                let stored = row
                    .model
                    .stored
                    .as_ref()
                    .map(|v| v.display())
                    .unwrap_or_default();
                let external = row
                    .model
                    .external
                    .as_ref()
                    .map(|v| v.display())
                    .unwrap_or_default();
                Some(
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_1()
                                .items_center()
                                .text_color(theme.warning)
                                .child(Icon::new(IconName::RefreshCw).small())
                                .child("Changed outside Licoman"),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Button::new(reload_id)
                                        .label("Reload")
                                        .outline()
                                        .xsmall()
                                        .on_click(cx.listener(
                                            move |this, _: &ClickEvent, _window, cx| {
                                                let setting_id = reload_setting_id.clone();
                                                this.app.update(cx, |state, cx| {
                                                    state.dispatch(
                                                        Command::Reload { id: setting_id },
                                                        cx,
                                                    );
                                                });
                                            },
                                        )),
                                )
                                .child(
                                    Button::new(compare_id)
                                        .label("Compare")
                                        .ghost()
                                        .xsmall()
                                        .on_click(cx.listener(
                                            move |_this, _: &ClickEvent, window, cx| {
                                                let stored = stored.clone();
                                                let external = external.clone();
                                                window.open_dialog(cx, move |dialog, _window, cx| {
                                                    dialog
                                                        .title("Compare changes")
                                                        .keyboard(true)
                                                        .child(
                                                            v_flex()
                                                                .gap_2()
                                                                .child(div().child(
                                                                    format!("Licoman (stored): {stored}"),
                                                                ))
                                                                .child(div().child(
                                                                    format!("Source (current): {external}"),
                                                                ))
                                                                .child(
                                                                    div()
                                                                        .text_color(cx.theme().muted_foreground)
                                                                        .text_size(px(metrics::TEXT_META))
                                                                        .child(
                                                                            "The source stays authoritative until you reload.",
                                                                        ),
                                                                ),
                                                        )
                                                });
                                            },
                                        )),
                                ),
                        ),
                )
                .into_any()
            }
        }
    }

    // ---- layout -----------------------------------------------------------

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let selected = self.app.read(cx).store.selected_scope();

        let mut menu = SidebarMenu::new().children([
            scope_item(Scope::Shared, IconName::Globe, selected, cx),
            scope_item(Scope::Desktop, IconName::LayoutDashboard, selected, cx),
            scope_item(Scope::Distribution, IconName::HardDrive, selected, cx),
        ]);

        // Applications is an action that opens/focuses the dedicated window.
        menu = menu.child(
            SidebarMenuItem::new("Applications")
                .icon(IconName::Inbox)
                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.open_applications_window(cx);
                })),
        );

        // Grounded environment summary (03_ENVIRONMENT_DISCOVERY.md UI
        // details): distro · desktop · protocol, each part omitted when
        // undetected, plus the live scan status. Partial results show as
        // they arrive — no pretending.
        let (summary, status_line) = {
            let state = self.app.read(cx);
            let summary = state
                .environment_snapshot()
                .map(|snapshot| snapshot.summary());
            let status_line = match state.scan_state() {
                ScanState::Loading { .. } => Some("Discovering environment…".to_string()),
                ScanState::Cancelled => Some("Scan cancelled — refreshing…".to_string()),
                ScanState::Idle => Some("Discovery pending…".to_string()),
                ScanState::Ready => None,
            };
            (summary, status_line)
        };

        // The sidebar footer's h_flex leaves children unconstrained; without
        // an explicit width a long summary renders as one clipped line.
        // Constrain to the sidebar content width (244 − 2×12 padding) so the
        // text wraps honestly instead of disappearing.
        let mut environment = v_flex()
            .w(px(metrics::SIDEBAR_WIDTH - 24.))
            .pb_3()
            .gap_1()
            .child(
                div()
                    .text_size(px(metrics::TEXT_META))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.muted_foreground)
                    .child("Environment"),
            )
            .child(
                div()
                    .text_size(px(metrics::TEXT_BODY))
                    .text_color(theme.foreground)
                    .child(summary.unwrap_or_else(|| "Discovering environment…".to_string())),
            );
        if let Some(status_line) = status_line {
            environment = environment.child(
                div()
                    .text_size(px(metrics::TEXT_META))
                    .text_color(theme.muted_foreground)
                    .child(status_line),
            );
        }

        Sidebar::new("scope-sidebar")
            .w(px(metrics::SIDEBAR_WIDTH))
            .collapsible(false)
            .child(menu)
            .footer(environment)
            .into_any_element()
    }

    fn render_content(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let scope = self.app.read(cx).store.selected_scope();
        let rows: Vec<Row> = self
            .app
            .read(cx)
            .store
            .scope_rows(scope)
            .into_iter()
            .map(|(descriptor, model)| Row { descriptor, model })
            .collect();

        let mut groups: Vec<AnyElement> = Vec::new();
        for category in self.app.read(cx).store.categories(scope) {
            let cat_rows: Vec<Row> = rows
                .iter()
                .filter(|row| row.descriptor.category == category)
                .cloned()
                .collect();
            let mut rendered: Vec<AnyElement> = Vec::new();
            for (index, row) in cat_rows.iter().enumerate() {
                rendered.push(self.render_setting(index, row, window, cx));
            }
            groups.push(settings_group(category, rendered, &theme).into_any_element());
        }

        let empty_state = rows.is_empty();
        v_flex()
            .flex_1()
            .min_h_0()
            .id("settings-content")
            .overflow_y_scroll()
            .px(px(metrics::CONTENT_PADDING))
            .py_5()
            .gap_6()
            .child(
                v_flex()
                    .max_w(px(metrics::CONTENT_MAX_WIDTH))
                    .gap_6()
                    .child(window_heading(scope.label(), &theme))
                    .child(
                        div()
                            .text_size(px(metrics::TEXT_BODY))
                            .text_color(theme.muted_foreground)
                            .child(scope.description()),
                    )
                    .when(!empty_state, |content| {
                        content.child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(
                                    gpui_kit::component::tag::Tag::secondary()
                                        .small()
                                        .child("Mock values"),
                                )
                                .child(
                                    div()
                                        .text_size(px(metrics::TEXT_META))
                                        .text_color(theme.muted_foreground)
                                        .child(
                                            "Rows come from built-in module manifests; mock-backed \
                                             values are samples. Real-backend rows stay read-only.",
                                        ),
                                ),
                        )
                    })
                    .when(empty_state, |content| {
                        // No providers registered (or none for this scope):
                        // a clear unavailable state, not a blank window.
                        content.child(
                            v_flex()
                                .gap_2()
                                .pt_6()
                                .child(
                                    div()
                                        .text_size(px(metrics::TEXT_LABEL))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(theme.foreground)
                                        .child("No settings available here"),
                                )
                                .child(
                                    div()
                                        .text_size(px(metrics::TEXT_BODY))
                                        .text_color(theme.muted_foreground)
                                        .child(
                                            "Licoman found no settings providers for this scope. Environments without supported providers stay usable and show this explanation instead of an empty pane.",
                                        ),
                                ),
                        )
                    })
                    .children(groups),
            )
            .into_any_element()
    }

    fn render_apply_bar(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme().clone();
        let (pending, applying) = {
            let store = &self.app.read(cx).store;
            (store.pending_count(), store.applying())
        };
        if pending == 0 && !applying {
            return None;
        }
        Some(
            h_flex()
                .justify_between()
                .items_center()
                .px(px(metrics::CONTENT_PADDING))
                .py_3()
                .border_t_1()
                .border_color(theme.border)
                .bg(theme.popover)
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Icon::new(IconName::CircleAlert)
                                .small()
                                .text_color(theme.warning),
                        )
                        .child(
                            div()
                                .text_size(px(metrics::TEXT_LABEL))
                                .text_color(theme.foreground)
                                .child(if applying {
                                    "Applying changes…".to_string()
                                } else {
                                    format!(
                                        "{pending} change{} pending",
                                        if pending == 1 { "" } else { "s" }
                                    )
                                }),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("discard-changes")
                                .label("Discard")
                                .ghost()
                                .disabled(applying)
                                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                                    this.app.update(cx, |state, cx| {
                                        state.dispatch(Command::DiscardAll, cx);
                                    });
                                })),
                        )
                        .child(
                            Button::new("apply-changes")
                                .label(if applying { "Applying…" } else { "Apply" })
                                .primary()
                                .loading(applying)
                                .disabled(applying)
                                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                                    this.app.update(cx, |state, cx| {
                                        state.dispatch(Command::ApplyAll, cx);
                                    });
                                })),
                        ),
                ),
        )
        .into_any()
    }

    fn render_palette(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.palette_open {
            return None;
        }
        let theme = cx.theme().clone();
        let results = self.palette_results(cx);
        let count = results.len();
        let selected = self.palette_selected.min(count.saturating_sub(1));

        let items: Vec<AnyElement> = results
            .iter()
            .enumerate()
            .map(|(i, hit)| {
                let label = hit.label.clone();
                let breadcrumb = hit.breadcrumb.clone();
                // Apply status next to the breadcrumb (04 §6): the row's
                // current phase, looked up by stable id.
                let status = {
                    let store = &self.app.read(cx).store;
                    store
                        .model(&hit.id)
                        .and_then(crate::ui::setting_controls::phase_status_text)
                };
                let is_selected = i == selected;
                div()
                    .id(SharedString::from(format!("palette-result-{i}")))
                    .px_3()
                    .py_2()
                    .rounded_sm()
                    .when(is_selected, |row| row.bg(theme.selection))
                    .hover(|row| row.bg(theme.accent))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.palette_selected = i;
                        this.accept_palette_result(cx);
                    }))
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_size(px(metrics::TEXT_LABEL))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(theme.foreground)
                                            .child(label),
                                    )
                                    .children(status.map(|text| {
                                        gpui_kit::component::tag::Tag::warning().small().child(text)
                                    })),
                            )
                            .child(
                                div()
                                    .text_size(px(metrics::TEXT_META))
                                    .text_color(theme.muted_foreground)
                                    .child(breadcrumb),
                            ),
                    )
                    .into_any_element()
            })
            .collect();

        let query_is_empty = self.palette_input.read(cx).value().trim().is_empty();
        let empty = count == 0;
        Some(
            h_flex()
                .absolute()
                .inset_0()
                .bg(theme.overlay)
                .key_context("SearchPalette")
                .items_start()
                .justify_center()
                .pt(px(64.))
                .on_action(cx.listener(|this, _: &DismissPalette, window, cx| {
                    this.close_palette(window, cx);
                }))
                .on_action(cx.listener(move |this, _: &PaletteNext, _window, cx| {
                    if this.palette_selected + 1 < count {
                        this.palette_selected += 1;
                        cx.notify();
                    }
                }))
                .on_action(cx.listener(|this, _: &PalettePrev, _window, cx| {
                    this.palette_selected = this.palette_selected.saturating_sub(1);
                    cx.notify();
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        // Clicking the backdrop dismisses; clicks on the panel
                        // stop propagation before reaching the backdrop.
                        this.close_palette(window, cx);
                    }),
                )
                .child(
                    v_flex()
                        .w(px(560.))
                        .max_h(px(420.))
                        .bg(theme.popover)
                        .border_1()
                        .border_color(theme.border)
                        .rounded(px(metrics::RADIUS_LG))
                        .overflow_hidden()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .child(
                            div()
                                .p_3()
                                .border_b_1()
                                .border_color(theme.border)
                                .child(
                                    Input::new(&self.palette_input)
                                        .id("palette-input")
                                        .appearance(false)
                                        .large(),
                                ),
                        )
                        .when(empty, |panel| {
                            panel.child(
                                v_flex()
                                    .p_6()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_size(px(metrics::TEXT_LABEL))
                                            .text_color(theme.foreground)
                                            .child(if query_is_empty {
                                                "Search all settings"
                                            } else {
                                                "No matching settings"
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(metrics::TEXT_BODY))
                                            .text_color(theme.muted_foreground)
                                            .child(if query_is_empty {
                                                "Type to search across every registered setting; results show scope → category → setting."
                                            } else {
                                                "Try a different search. Built-in modules cover Shared, Desktop, Distribution and Applications."
                                            }),
                                    ),
                            )
                        })
                        .when(!empty, |panel| panel.children(items)),
                ),
        )
        .into_any()
    }

    fn open_applications_window(&mut self, cx: &mut Context<Self>) {
        let app = self.app.clone();
        open_or_focus(
            WindowRole::Applications,
            move |cx| crate::windows::open_apps_window(&app, cx),
            cx,
        );
    }

    fn open_preferences_window(&mut self, cx: &mut Context<Self>) {
        let app = self.app.clone();
        open_or_focus(
            WindowRole::Preferences,
            move |cx| crate::windows::open_preferences_window(&app, cx),
            cx,
        );
    }
}

fn model_restart_target(model: &SettingModel) -> Option<String> {
    if model.restart_required {
        Some("the affected program".to_string())
    } else {
        None
    }
}

fn scope_item(
    scope: Scope,
    icon: IconName,
    selected: Scope,
    cx: &mut Context<MainWindow>,
) -> SidebarMenuItem {
    let active = scope == selected;
    SidebarMenuItem::new(scope.label())
        .icon(icon)
        .active(active)
        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
            this.app.update(cx, |state, cx| {
                state.dispatch(Command::SelectScope(scope), cx);
                state.push_history(WindowRole::Main, format!("scope:{}", scope.label()), cx);
            });
        }))
}

impl Render for MainWindow {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        if self.rebuild_needed {
            self.rebuild_controls(window, cx);
        } else if self.resync_needed {
            self.sync_owned_controls(window, cx);
        }
        if let Some(query) = self.pending_palette_query.take() {
            self.palette_open = true;
            self.palette_selected = 0;
            let focus = self.palette_input.read(cx).focus_handle(cx).clone();
            self.palette_input.update(cx, |state, cx| {
                state.set_value(query, window, cx);
            });
            window.focus(&focus, cx);
        }

        let theme = cx.theme().clone();

        v_flex()
            .key_context("MainWindow")
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .on_action(cx.listener(|this, _: &OpenSearch, window, cx| {
                this.open_palette(window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                let handle = this.header_search.read(cx).focus_handle(cx).clone();
                window.focus(&handle, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenPreferences, _window, cx| {
                this.open_preferences_window(cx);
            }))
            .on_action(cx.listener(|this, _: &OpenApplications, _window, cx| {
                this.open_applications_window(cx);
            }))
            .on_action(cx.listener(|this, _: &DismissPalette, window, cx| {
                if this.palette_open {
                    this.close_palette(window, cx);
                }
            }))
            .child(
                TitleBar::new().h(px(metrics::HEADER_HEIGHT)).child(
                    h_flex()
                        .w_full()
                        .h_full()
                        .items_center()
                        .gap_3()
                        .px_2()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(
                                    Icon::new(gpui_kit::assets::IconName::SlidersHorizontal)
                                        .text_color(theme.primary),
                                )
                                .child(
                                    div()
                                        .text_size(px(metrics::TEXT_LABEL))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child("Licoman"),
                                ),
                        )
                        .child(
                            h_flex().flex_1().max_w(px(420.)).child(
                                Input::new(&self.header_search)
                                    .id("header-search")
                                    .prefix(Icon::new(IconName::Search).small())
                                    .cleanable(true),
                            ),
                        )
                        .child(
                            div()
                                .text_size(px(metrics::TEXT_META))
                                .text_color(theme.muted_foreground)
                                .child("Ctrl K"),
                        )
                        .child(
                            Button::new("open-preferences")
                                .icon(IconName::Settings)
                                .ghost()
                                .compact()
                                .tooltip("Licoman Preferences (Ctrl+,)")
                                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                                    this.open_preferences_window(cx);
                                })),
                        ),
                ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(self.render_sidebar(cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_h_0()
                            .border_l_1()
                            .border_color(theme.border)
                            .bg(theme.background)
                            .child(self.render_content(window, cx))
                            .children(self.render_apply_bar(cx)),
                    ),
            )
            .children(self.render_palette(cx))
    }
}
