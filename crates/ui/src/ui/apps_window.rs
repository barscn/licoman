//! Applications window (03_ENVIRONMENT_DISCOVERY.md UI details, registry-
//! driven since Phase 04): discovered applications merged with registered
//! application modules from the module registry. Two views — **Supported**
//! (module-backed) and **All installed** (everything discovered). Detail
//! panels are built from the module's settings through the shared
//! [`ControlSet`] factory; "Source details" shows recorded provenance.
//! Unknown apps show the honest "No Licoman integration available yet."
//! message and never fake settings.

use gpui_kit::component::TitleBar;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::component::{Icon, IconName, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, ParentElement as _, Render,
    SharedString, Styled, Subscription, Window, div, px,
};
use gpui_kit::{
    Focusable as _, InteractiveElement as _, IntoElement as _, StatefulInteractiveElement as _,
};
use licoman_core::descriptor::SettingDescriptor;
use licoman_core::discovery::AppListEntry;
use licoman_core::prefs::AppearanceMode;
use licoman_core::{Command, SettingModel, WindowRole};

use crate::actions::*;
use crate::app_state::{AppEvent, AppState, ScanState, open_or_focus};
use crate::theme::metrics;
use crate::ui::setting_controls::{ControlSet, phase_status_text};
use crate::ui::{OptionIntoAny as _, setting_row, support_tag};

/// The exact honesty message required by the plan.
const NO_INTEGRATION: &str = "No Licoman integration available yet.";

pub struct AppsWindow {
    app: Entity<AppState>,
    filter: Entity<InputState>,
    /// `false` = Supported (module-backed), `true` = All installed.
    show_all: bool,
    selected_tab: usize,
    /// Registry-driven controls for every application-scope setting.
    controls: ControlSet,
    /// Set when the store changed; applied at the next render, where window
    /// access is available for control re-sync.
    resync_needed: bool,
    /// Set when module applicability changed; rebuild, don't just sync.
    rebuild_needed: bool,
    /// True while background rows are still unread; the transition to
    /// all-read triggers one control rebuild.
    had_pending_reads: bool,
    _subscriptions: Vec<Subscription>,
    _appearance_sub: Option<Subscription>,
}

impl AppsWindow {
    pub fn new(app: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter apps…"));

        // Controls for every application-scope setting (all app modules);
        // the detail pane renders the subset of the selected module.
        let mut app_rows: Vec<(SettingDescriptor, SettingModel)> = Vec::new();
        {
            let store = &app.read(cx).store;
            let scope = licoman_core::descriptor::Scope::Application;
            app_rows.extend(store.scope_rows(scope));
        }
        let controls = ControlSet::build(&app, &app_rows, window, cx);

        let mut subscriptions = Vec::new();
        subscriptions.push(cx.subscribe(&filter, {
            move |_this, _input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change | InputEvent::PressEnter { .. }) {
                    cx.notify();
                }
            }
        }));
        subscriptions.push(cx.subscribe(&app, {
            move |this, _app, event: &AppEvent, cx| {
                match event {
                    AppEvent::PreferencesChanged => cx.notify(),
                    AppEvent::SettingsChanged => {
                        // The first read pass moves rows from Unknown to
                        // read (and, in fixture-write mode, to writable):
                        // the control set must be rebuilt then.
                        let pending = this.app.read(cx).store.background_rows_pending();
                        if this.had_pending_reads && !pending {
                            this.rebuild_needed = true;
                        }
                        this.had_pending_reads = pending;
                        this.resync_needed = true;
                        cx.notify();
                    }
                    AppEvent::ModulesChanged => {
                        this.rebuild_needed = true;
                        this.resync_needed = false;
                        cx.notify();
                    }
                    AppEvent::EnvironmentChanged => {
                        // Default selection: the first supported entry once a
                        // scan has produced one. Stable id, not an index.
                        let first = {
                            let state = this.app.read(cx);
                            let needs_default = state.store.selected_app().is_none()
                                && matches!(state.scan_state(), ScanState::Ready);
                            if needs_default {
                                state
                                    .app_list()
                                    .iter()
                                    .find(|entry| entry.visible_as_supported())
                                    .or_else(|| {
                                        state.app_list().iter().find(|entry| entry.app.is_some())
                                    })
                                    .map(|entry| entry.identity.clone())
                            } else {
                                None
                            }
                        };
                        if let Some(id) = first {
                            this.app.update(cx, |state, cx| {
                                state.dispatch(Command::SelectApp { id: Some(id) }, cx);
                            });
                        }
                        cx.notify();
                    }
                    // The Applications window shows no account state.
                    AppEvent::AccountChanged => {}
                }
            }
        }));

        // Follow desktop appearance while `System` is selected.
        let appearance_app = app.clone();
        let appearance_sub = window.observe_window_appearance(move |_window, cx| {
            appearance_app.update(cx, |state, cx| {
                if state.store.preferences().theme == AppearanceMode::System {
                    state.resolve_and_apply_theme(cx);
                }
            });
        });

        Self {
            app,
            filter,
            show_all: false,
            selected_tab: 0,
            controls,
            resync_needed: false,
            rebuild_needed: false,
            had_pending_reads: true,
            _subscriptions: subscriptions,
            _appearance_sub: Some(appearance_sub),
        }
    }

    /// Push store values into owned controls after store changes.
    fn resync_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.resync_needed = false;
        let mut app_rows: Vec<(SettingDescriptor, SettingModel)> = Vec::new();
        {
            let store = &self.app.read(cx).store;
            let scope = licoman_core::descriptor::Scope::Application;
            app_rows.extend(store.scope_rows(scope));
        }
        self.controls.sync(&app_rows, window, cx);
    }

    /// Rebuild the control set after module applicability changed.
    fn rebuild_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rebuild_needed = false;
        let mut app_rows: Vec<(SettingDescriptor, SettingModel)> = Vec::new();
        {
            let store = &self.app.read(cx).store;
            let scope = licoman_core::descriptor::Scope::Application;
            app_rows.extend(store.scope_rows(scope));
        }
        self.controls.rebuild(&self.app, &app_rows, window, cx);
    }

    /// The merged list narrowed by the current view and the text filter.
    fn visible_entries(&self, cx: &Context<Self>) -> Vec<AppListEntry> {
        let query = self.filter.read(cx).value().to_lowercase();
        self.app
            .read(cx)
            .app_list()
            .iter()
            .filter(|entry| {
                if self.show_all {
                    entry.app.is_some()
                } else {
                    entry.visible_as_supported()
                }
            })
            .filter(|entry| query.is_empty() || entry.display_name.to_lowercase().contains(&query))
            .cloned()
            .collect()
    }

    fn render_list(&mut self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let theme = cx.theme().clone();
        let selected = self.app.read(cx).store.selected_app().map(str::to_string);
        let entries = self.visible_entries(cx);
        let items: Vec<gpui_kit::AnyElement> = entries
            .iter()
            .map(|entry| {
                let is_selected = selected.as_deref() == Some(entry.identity.as_str());
                let identity = entry.identity.clone();
                let subtitle = row_subtitle(entry);
                div()
                    .id(SharedString::from(format!("app-{}", entry.identity)))
                    .px_3()
                    .py_2()
                    .rounded_sm()
                    .when(is_selected, |row| row.bg(theme.selection))
                    .hover(|row| row.bg(theme.accent))
                    .on_click(
                        cx.listener(move |this, _: &gpui_kit::ClickEvent, _window, cx| {
                            let identity = identity.clone();
                            this.app.update(cx, |state, cx| {
                                state.dispatch(Command::SelectApp { id: Some(identity) }, cx);
                                let id = state.store.selected_app().unwrap_or_default();
                                state.push_history(
                                    WindowRole::Applications,
                                    format!("app:{id}"),
                                    cx,
                                );
                            });
                            this.selected_tab = 0;
                            cx.notify();
                        }),
                    )
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(metrics::TEXT_LABEL))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.foreground)
                                    .child(entry.display_name.clone()),
                            )
                            .child(support_tag(entry.support)),
                    )
                    .child(
                        div()
                            .text_size(px(metrics::TEXT_META))
                            .text_color(theme.muted_foreground)
                            .child(subtitle),
                    )
                    .into_any_element()
            })
            .collect();

        let scan = self.app.read(cx).scan_state();
        let empty = items.is_empty();
        v_flex()
            .w(px(metrics::APPS_LIST_WIDTH))
            .flex_shrink_0()
            .h_full()
            .border_r_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .p_2()
            .gap_2()
            // View switch: Supported (default) vs. All installed.
            .child(
                h_flex()
                    .gap_1()
                    .child(view_pill(
                        "view-supported",
                        "Supported",
                        !self.show_all,
                        &theme,
                        {
                            cx.listener(move |this, _: &gpui_kit::ClickEvent, _window, cx| {
                                this.show_all = false;
                                cx.notify();
                            })
                        },
                    ))
                    .child(view_pill(
                        "view-all",
                        "All installed",
                        self.show_all,
                        &theme,
                        {
                            cx.listener(move |this, _: &gpui_kit::ClickEvent, _window, cx| {
                                this.show_all = true;
                                cx.notify();
                            })
                        },
                    )),
            )
            .child(
                Input::new(&self.filter)
                    .id("app-filter")
                    .small()
                    .cleanable(true),
            )
            .when(matches!(scan, ScanState::Loading { .. }) && empty, |list| {
                list.child(
                    v_flex()
                        .pt_8()
                        .items_center()
                        .gap_2()
                        .child(
                            Icon::new(IconName::LoaderCircle)
                                .large()
                                .text_color(theme.muted_foreground),
                        )
                        .child(
                            div()
                                .text_size(px(metrics::TEXT_BODY))
                                .text_color(theme.foreground)
                                .child("Discovering applications…"),
                        )
                        .child(
                            div()
                                .text_size(px(metrics::TEXT_META))
                                .text_color(theme.muted_foreground)
                                .child("Application directories are scanned read-only."),
                        ),
                )
            })
            .when(
                !matches!(scan, ScanState::Loading { .. }) && empty,
                |list| {
                    list.child(
                        v_flex()
                            .pt_8()
                            .items_center()
                            .gap_2()
                            .child(
                                Icon::new(IconName::Inbox)
                                    .large()
                                    .text_color(theme.muted_foreground),
                            )
                            .child(
                                div()
                                    .text_size(px(metrics::TEXT_BODY))
                                    .text_color(theme.foreground)
                                    .child("No applications match"),
                            )
                            .child(
                                div()
                                    .text_size(px(metrics::TEXT_META))
                                    .text_color(theme.muted_foreground)
                                    .child("Adjust the filter or the view switch."),
                            ),
                    )
                },
            )
            .when(!empty, |list| list.children(items))
            .into_any_element()
    }

    fn render_detail(&mut self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let theme = cx.theme().clone();
        let selected = self.app.read(cx).store.selected_app().map(str::to_string);
        let Some(selected) = selected else {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child("Select an application to see its settings."),
                )
                .into_any_element();
        };
        let Some(entry) = self
            .app
            .read(cx)
            .app_list()
            .iter()
            .find(|entry| entry.identity == selected)
            .cloned()
        else {
            // Removed mid-session: graceful fallback, no stale UI state, no
            // panic (04_MODULE_SCHEMA_AND_REGISTRY.md §4).
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child("The selected application is no longer in the list."),
                )
                .into_any_element();
        };

        match (entry.module.clone(), entry.app.clone()) {
            // Registered module + installed app: registry-driven rows.
            (Some(module_id), Some(app)) => {
                self.render_module_detail(entry, &module_id, Some(&app), cx)
            }
            // Module row without an installed app.
            (Some(module_id), None) => self.render_not_installed_detail(entry, &module_id, cx),
            // Discovered but no module: the honest message and facts only.
            (None, _) => self.render_unsupported_detail(entry, cx),
        }
    }

    /// Grouped rows for one module: settings ordered as registered, grouped
    /// by their declared group/tab name.
    fn module_groups(
        &self,
        module_id: &str,
        cx: &Context<Self>,
    ) -> Vec<(String, Vec<(SettingDescriptor, SettingModel)>)> {
        let (ids, groups) = self
            .app
            .read(cx)
            .module_provider()
            .with_registry(|registry| {
                let ids = registry.setting_ids_of_module(module_id);
                let groups: Vec<String> = ids
                    .iter()
                    .filter_map(|id| registry.setting(id))
                    .map(|(def, setting)| def.setting_group(setting))
                    .collect();
                (ids, groups)
            });
        let rows = self.app.read(cx).store.rows_for_ids(&ids);
        let mut grouped: Vec<(String, Vec<(SettingDescriptor, SettingModel)>)> = Vec::new();
        for (group, row) in groups.into_iter().zip(rows) {
            match grouped.last_mut() {
                Some((current, rows)) if *current == group => rows.push(row),
                _ => grouped.push((group, vec![row])),
            }
        }
        grouped
    }

    fn render_module_detail(
        &mut self,
        entry: AppListEntry,
        module_id: &str,
        app: Option<&licoman_core::discovery::InstalledApp>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let theme = cx.theme().clone();
        let (title, description, provenance) =
            self.app
                .read(cx)
                .module_provider()
                .with_registry(|registry| match registry.module(module_id) {
                    Some(module) => (
                        module.def.title.clone(),
                        module.def.description.clone(),
                        registry.provenance_of(module_id),
                    ),
                    None => (entry.display_name.clone(), String::new(), Vec::new()),
                });
        let display_title = app
            .map(|app| app.name.clone())
            .unwrap_or_else(|| title.clone());
        let description = match app {
            Some(app) => app.comment.clone().unwrap_or_else(|| description.clone()),
            None => description.clone(),
        };

        let groups = self.module_groups(module_id, cx);
        if self.selected_tab >= groups.len() {
            self.selected_tab = 0;
        }

        let mut header = v_flex().gap_1().child(
            h_flex()
                .gap_3()
                .items_center()
                .child(
                    div()
                        .text_size(px(metrics::TEXT_PAGE_HEADING))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child(display_title),
                )
                .child(support_tag(entry.support)),
        );
        if !description.is_empty() {
            header = header.child(
                div()
                    .text_size(px(metrics::TEXT_BODY))
                    .text_color(theme.muted_foreground)
                    .child(description),
            );
        }
        // Install-origin + backend facts as a meta line.
        if let Some(app) = app {
            header = header.child(
                div()
                    .text_size(px(metrics::TEXT_META))
                    .text_color(theme.muted_foreground)
                    .child(format!(
                        "Installed via {} · {}",
                        app.origin.label(),
                        if entry.backend_available {
                            "configuration location detected"
                        } else {
                            "no configuration location found yet"
                        },
                    )),
            );
        }

        // Tab bar: one pill per declared settings group.
        let tab_bar = h_flex()
            .gap_1()
            .flex_wrap()
            .children(groups.iter().enumerate().map(|(i, (name, _))| {
                let is_active = i == self.selected_tab;
                let name = name.clone();
                div()
                    .id(SharedString::from(format!("tab-{}-{i}", entry.identity)))
                    .px_3()
                    .py_1()
                    .rounded(px(metrics::RADIUS_SM))
                    .text_size(px(metrics::TEXT_BODY))
                    .when(is_active, |tab| {
                        tab.bg(theme.selection)
                            .text_color(theme.foreground)
                            .font_weight(FontWeight::MEDIUM)
                    })
                    .when(!is_active, |tab| tab.text_color(theme.muted_foreground))
                    .hover(|tab| tab.bg(theme.accent))
                    .on_click(
                        cx.listener(move |this, _: &gpui_kit::ClickEvent, _window, cx| {
                            this.selected_tab = i;
                            cx.notify();
                        }),
                    )
                    .child(name)
            }));

        // The selected group's rows, registry-driven through the ControlSet.
        let body: gpui_kit::AnyElement = match groups.get(self.selected_tab) {
            None => v_flex()
                .gap_2()
                .pt_4()
                .child(
                    div()
                        .text_size(px(metrics::TEXT_LABEL))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.foreground)
                        .child(NO_INTEGRATION),
                )
                .into_any_element(),
            Some((group_name, rows)) => {
                let rendered: Vec<AnyElement> = rows
                    .iter()
                    .enumerate()
                    .map(|(index, (descriptor, model))| {
                        let control = self.controls.render(descriptor, model, &self.app, &theme);
                        let status = phase_status_text(model).map(|text| {
                            div()
                                .text_color(
                                    if model.phase == licoman_core::SettingPhase::Failed
                                        || model.phase
                                            == licoman_core::SettingPhase::ExternalConflict
                                    {
                                        theme.warning
                                    } else {
                                        theme.muted_foreground
                                    },
                                )
                                .child(text)
                        });
                        setting_row(
                            SharedString::from(format!("app-setting-{index}")),
                            descriptor.label.clone(),
                            descriptor.description.clone(),
                            control,
                            status.into_any(),
                            &theme,
                        )
                        .into_any_element()
                    })
                    .collect();
                v_flex()
                    .gap_6()
                    .pt_4()
                    .child(crate::ui::settings_group(group_name.clone(), rendered, &theme))
                    .child({
                        let provenance = provenance.clone();
                        let module_title = title.clone();
                        h_flex().gap_2().child(
                            Button::new("source-details")
                                .label("Source details…")
                                .outline()
                                .small()
                                .on_click(cx.listener(move |_this, _: &gpui_kit::ClickEvent, window, cx| {
                                    let provenance = provenance.clone();
                                    let module_title = module_title.clone();
                                    window.open_dialog(cx, move |dialog, _window, cx| {
                                        let mut panel = v_flex().gap_2().child(
                                            div()
                                                .text_size(px(metrics::TEXT_BODY))
                                                .text_color(cx.theme().foreground)
                                                .child(format!(
                                                    "Module {module_title} — recorded provenance"
                                                )),
                                        );
                                        if provenance.is_empty() {
                                            panel = panel.child(
                                                div()
                                                    .text_size(px(metrics::TEXT_META))
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(
                                                        "Mock values only — no external backend binding.",
                                                    ),
                                            );
                                        }
                                        for line in provenance.clone() {
                                            panel = panel.child(
                                                div()
                                                    .font_family("monospace")
                                                    .text_size(px(metrics::TEXT_META))
                                                    .child(line),
                                            );
                                        }
                                        panel = panel.child(
                                            div()
                                                .text_size(px(metrics::TEXT_META))
                                                .text_color(cx.theme().muted_foreground)
                                                .child(
                                                    "Bindings are validated declaratively; reads arrive \
                                                     with Phase 05 and writes are gated behind the \
                                                     safe write engine (Phase 06).",
                                                ),
                                        );
                                        dialog.title("Source details").keyboard(true).child(panel)
                                    });
                                })),
                        )
                    })
                    .into_any_element()
            }
        };

        v_flex()
            .flex_1()
            .min_h_0()
            .id("apps-detail")
            .overflow_y_scrollbar()
            .px(px(metrics::CONTENT_PADDING))
            .py_5()
            .max_w(px(
                metrics::CONTENT_MAX_WIDTH + metrics::CONTENT_PADDING * 2.
            ))
            .child(header)
            .when(groups.len() > 1, |detail| detail.child(tab_bar))
            .child(body)
            .into_any_element()
    }

    fn render_unsupported_detail(
        &mut self,
        entry: AppListEntry,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let theme = cx.theme().clone();
        let mut facts = v_flex().gap_1().pt_2();
        if let Some(app) = &entry.app {
            facts = facts
                .child(fact_row("Application id", app.id.clone(), &theme))
                .child(fact_row(
                    "Install origin",
                    app.origin.label().to_string(),
                    &theme,
                ));
            if let Some(flatpak) = &app.flatpak_id {
                facts = facts.child(fact_row("Flatpak", flatpak.clone(), &theme));
            }
            facts = facts.child(fact_row(
                "Configuration location",
                if entry.backend_available {
                    "Detected (read-only probe)".to_string()
                } else {
                    "Not found yet".to_string()
                },
                &theme,
            ));
        }
        v_flex()
            .flex_1()
            .min_h_0()
            .id("apps-detail")
            .overflow_y_scrollbar()
            .px(px(metrics::CONTENT_PADDING))
            .py_5()
            .max_w(px(
                metrics::CONTENT_MAX_WIDTH + metrics::CONTENT_PADDING * 2.
            ))
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(
                                div()
                                    .text_size(px(metrics::TEXT_PAGE_HEADING))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.foreground)
                                    .child(entry.display_name.clone()),
                            )
                            .child(support_tag(entry.support)),
                    )
                    .child(
                        div()
                            .text_size(px(metrics::TEXT_LABEL))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.foreground)
                            .pt_4()
                            .child(NO_INTEGRATION),
                    )
                    .child(
                        div()
                            .text_size(px(metrics::TEXT_BODY))
                            .text_color(theme.muted_foreground)
                            .child(
                                "Licoman does not know any settings for this application, so it \
                                 is listed as unsupported rather than pretending to configure it.",
                            ),
                    )
                    .child(facts),
            )
            .into_any_element()
    }

    fn render_not_installed_detail(
        &mut self,
        entry: AppListEntry,
        module_id: &str,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let theme = cx.theme().clone();
        let (title, description) = self
            .app
            .read(cx)
            .module_provider()
            .with_registry(|registry| match registry.module(module_id) {
                Some(module) => (module.def.title.clone(), module.def.description.clone()),
                None => (entry.display_name.clone(), String::new()),
            });
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_2()
            .child(
                div()
                    .text_size(px(metrics::TEXT_PAGE_HEADING))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.foreground)
                    .child(title),
            )
            .child(support_tag(entry.support))
            .child(
                div()
                    .text_size(px(metrics::TEXT_BODY))
                    .text_color(theme.muted_foreground)
                    .max_w(px(metrics::CONTENT_MAX_WIDTH))
                    .child(if description.is_empty() {
                        "Licoman has a module registered, but the application was not detected \
                         on this system."
                            .to_string()
                    } else {
                        description
                    }),
            )
            .into_any_element()
    }

    /// Visual-verification hook (`LICOMAN_DEMO=all-apps`): open the window
    /// directly in the "All installed" view.
    pub fn demo_show_all(&mut self) {
        self.show_all = true;
    }
}

/// One-line list subtitle: what we actually know about the row.
fn row_subtitle(entry: &AppListEntry) -> String {
    match &entry.app {
        Some(app) => app
            .comment
            .clone()
            .or_else(|| app.generic_name.clone())
            .unwrap_or_else(|| format!("Installed via {}", app.origin.label())),
        None => "Module registered; application not detected".to_string(),
    }
}

fn fact_row(
    label: &str,
    value: String,
    theme: &gpui_kit::component::Theme,
) -> gpui_kit::AnyElement {
    h_flex()
        .gap_2()
        .child(
            div()
                .w(px(180.))
                .text_size(px(metrics::TEXT_META))
                .text_color(theme.muted_foreground)
                .child(label.to_string()),
        )
        .child(
            div()
                .font_family("monospace")
                .text_size(px(metrics::TEXT_META))
                .text_color(theme.foreground)
                .child(value),
        )
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn view_pill(
    id: &'static str,
    label: &'static str,
    active: bool,
    theme: &gpui_kit::component::Theme,
    on_click: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
) -> gpui_kit::AnyElement {
    div()
        .id(SharedString::from(id))
        .px_3()
        .py_1()
        .rounded(px(metrics::RADIUS_SM))
        .text_size(px(metrics::TEXT_BODY))
        .when(active, |pill| {
            pill.bg(theme.selection)
                .text_color(theme.foreground)
                .font_weight(FontWeight::MEDIUM)
        })
        .when(!active, |pill| pill.text_color(theme.muted_foreground))
        .hover(|pill| pill.bg(theme.accent))
        .on_click(on_click)
        .child(label)
        .into_any_element()
}

impl Render for AppsWindow {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        if self.rebuild_needed {
            self.rebuild_controls(window, cx);
        } else if self.resync_needed {
            self.resync_controls(window, cx);
        }
        let theme = cx.theme().clone();
        let (subtitle, refreshing) = {
            let state = self.app.read(cx);
            let apps = state.environment_snapshot().map(|s| s.installed_apps.len());
            let modules = state
                .app_list()
                .iter()
                .filter(|e| e.module.is_some())
                .count();
            match state.scan_state() {
                ScanState::Loading { stage } => (
                    match stage {
                        licoman_core::discovery::ScanStage::Applications => {
                            "Scanning application directories…".to_string()
                        }
                        _ => "Discovering environment…".to_string(),
                    },
                    true,
                ),
                ScanState::Cancelled => ("Discovery cancelled — refreshing…".to_string(), true),
                ScanState::Idle => ("Environment discovery pending…".to_string(), true),
                ScanState::Ready => (
                    match apps {
                        Some(count) => format!(
                            "{count} applications discovered · {modules} with Licoman modules"
                        ),
                        None => "Environment ready".to_string(),
                    },
                    false,
                ),
            }
        };

        v_flex()
            .key_context("AppsWindow")
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                let handle = this.filter.read(cx).focus_handle(cx).clone();
                window.focus(&handle, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenPreferences, _window, cx| {
                let app = this.app.clone();
                open_or_focus(
                    WindowRole::Preferences,
                    move |cx| crate::windows::open_preferences_window(&app, cx),
                    cx,
                );
            }))
            .on_action(cx.listener(|_this, _: &OpenApplications, _window, _cx| {
                // Already in the Applications window; nothing to open.
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
                            div()
                                .text_size(px(metrics::TEXT_LABEL))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Applications"),
                        )
                        .child(
                            div()
                                .text_size(px(metrics::TEXT_META))
                                .text_color(theme.muted_foreground)
                                .child(subtitle),
                        )
                        .when(!refreshing, |bar| {
                            bar.child(
                                Button::new("refresh-discovery")
                                    .label("Refresh")
                                    .icon(IconName::RefreshCw)
                                    .small()
                                    .outline()
                                    .on_click(cx.listener(
                                        |this, _: &gpui_kit::ClickEvent, _window, cx| {
                                            this.app.update(cx, |state, cx| {
                                                state.refresh_environment(cx)
                                            });
                                        },
                                    )),
                            )
                        }),
                ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(self.render_list(cx))
                    .child(self.render_detail(cx)),
            )
    }
}
