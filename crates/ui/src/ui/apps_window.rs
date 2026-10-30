//! Applications window (03_ENVIRONMENT_DISCOVERY.md UI details): discovered
//! applications merged with the registered mock modules. Two views —
//! **Supported** (module-backed) and **All installed** (everything
//! discovered). Unknown apps show the honest "No Licoman integration
//! available yet." message and never fake settings. Discovery runs off the
//! UI thread; this window renders loading/partial states and a Refresh
//! action.

use gpui_kit::component::TitleBar;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputEvent, InputState, NumberInput};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState, SliderValue};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::component::{Icon, IconName, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, FontWeight, ParentElement as _, Render, SharedString, Styled,
    Subscription, Window, div, px,
};
use gpui_kit::{
    Focusable as _, InteractiveElement as _, IntoElement as _, StatefulInteractiveElement as _,
};
use licoman_core::discovery::AppListEntry;
use licoman_core::mock::Support;
use licoman_core::prefs::AppearanceMode;
use licoman_core::{Command, WindowRole};

use crate::actions::*;
use crate::app_state::{AppEvent, AppState, ScanState, open_or_focus};
use crate::theme::metrics;
use crate::ui::{OptionIntoAny as _, settings_group, support_tag};

/// The exact honesty message required by the plan.
const NO_INTEGRATION: &str = "No Licoman integration available yet.";

pub struct AppsWindow {
    app: Entity<AppState>,
    filter: Entity<InputState>,
    /// `false` = Supported (module-backed), `true` = All installed.
    show_all: bool,
    font_size: Entity<InputState>,
    opacity: Entity<SliderState>,
    cursor_blink: bool,
    selected_tab: usize,
    _subscriptions: Vec<Subscription>,
    _appearance_sub: Option<Subscription>,
}

impl AppsWindow {
    pub fn new(app: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter apps…"));
        let font_size = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("13")
                .step(0.5)
                .min(6.)
                .max(32.)
                .default_value("13")
        });
        let opacity = cx.new(|_| {
            SliderState::new()
                .min(30.)
                .max(100.)
                .step(5.)
                .default_value(95.)
        });

        let mut subscriptions = Vec::new();
        let opacity_for_event = opacity.clone();
        subscriptions.push(cx.subscribe(&opacity, {
            move |_this, _slider, event: &SliderEvent, cx| {
                if let SliderEvent::Change(SliderValue::Single(v)) = event {
                    let _ = v;
                    cx.notify();
                }
            }
        }));
        subscriptions.push(cx.subscribe(&font_size, {
            move |_this, _input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }
        }));
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
                    AppEvent::PreferencesChanged | AppEvent::SettingsChanged => cx.notify(),
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
            font_size,
            opacity: opacity_for_event,
            cursor_blink: true,
            selected_tab: 0,
            _subscriptions: subscriptions,
            _appearance_sub: Some(appearance_sub),
        }
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

        match (entry.module, entry.app.clone()) {
            // Registered module + installed app: sample controls, honestly
            // labeled, with discovered facts attached.
            (Some(module_id), Some(app)) => {
                self.render_module_detail(entry, module_id, Some(&app), cx)
            }
            // Module row without an installed app.
            (Some(module_id), None) => self.render_not_installed_detail(entry, module_id, cx),
            // Discovered but no module: the honest message and facts only.
            (None, _) => self.render_unsupported_detail(entry, cx),
        }
    }

    fn render_module_detail(
        &mut self,
        entry: AppListEntry,
        module_id: &str,
        app: Option<&licoman_core::discovery::InstalledApp>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let theme = cx.theme().clone();
        let catalog = licoman_core::mock::APPS_CATALOG
            .iter()
            .find(|m| m.id == module_id);
        let (title, description, source, categories) = match (app, catalog) {
            (Some(app), Some(module)) => (
                app.name.clone(),
                app.comment
                    .clone()
                    .unwrap_or_else(|| module.description.to_string()),
                redacted_source(app),
                module.categories,
            ),
            (_, Some(module)) => (
                module.name.to_string(),
                module.description.to_string(),
                module.source.to_string(),
                module.categories,
            ),
            (Some(app), None) => (
                app.name.clone(),
                app.comment.clone().unwrap_or_default(),
                redacted_source(app),
                &[] as &'static [(&'static str, &'static str)],
            ),
            (None, None) => (
                entry.display_name.clone(),
                String::new(),
                String::new(),
                &[] as &'static [(&'static str, &'static str)],
            ),
        };

        let tabs = categories;
        if self.selected_tab >= tabs.len() {
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
                        .child(title),
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

        // Tab bar (custom pills; gpui-kit tabs are heavier than needed here).
        let tab_bar = h_flex()
            .gap_1()
            .flex_wrap()
            .children(tabs.iter().enumerate().map(|(i, (name, _))| {
                let is_active = i == self.selected_tab;
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
                    .child(*name)
            }));

        let body: gpui_kit::AnyElement = match entry.support {
            Support::Unsupported => v_flex()
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
            Support::Partial => {
                let tab_description = tabs.get(self.selected_tab).map(|(_, d)| *d).unwrap_or("");
                let source = source.clone();
                let rows = vec![
                    self.render_generic_row_font_size(cx),
                    self.render_config_source_row(source, cx),
                ];
                v_flex()
                    .gap_6()
                    .pt_4()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(Icon::new(IconName::Info).small().text_color(theme.info))
                            .child(
                                div()
                                    .text_size(px(metrics::TEXT_BODY))
                                    .text_color(theme.muted_foreground)
                                    .child("Partial support — a subset of options is available."),
                            ),
                    )
                    .child(settings_group(tabs[self.selected_tab].0, rows, &theme))
                    .child(
                        div()
                            .text_size(px(metrics::TEXT_META))
                            .text_color(theme.muted_foreground)
                            .child(tab_description),
                    )
                    .into_any_element()
            }
            Support::Supported => {
                let source = source.clone();
                let rows = vec![
                    self.render_generic_row_font_size(cx),
                    self.render_opacity_row(cx),
                    self.render_cursor_blink_row(cx),
                    self.render_config_source_row(source, cx),
                ];
                v_flex()
                    .gap_6()
                    .pt_4()
                    .child(settings_group(
                        tabs.get(self.selected_tab)
                            .map(|(n, _)| *n)
                            .unwrap_or("Settings"),
                        rows,
                        &theme,
                    ))
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
            .when(tabs.len() > 1, |detail| detail.child(tab_bar))
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
        let module = licoman_core::mock::APPS_CATALOG
            .iter()
            .find(|m| m.id == module_id);
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
                    .child(entry.display_name.clone()),
            )
            .child(support_tag(entry.support))
            .child(
                div()
                    .text_size(px(metrics::TEXT_BODY))
                    .text_color(theme.muted_foreground)
                    .child(format!(
                        "Licoman has a {} module registered, but the application was not \
                         detected on this system.",
                        module.map(|m| m.name).unwrap_or(module_id)
                    )),
            )
            .into_any_element()
    }

    fn render_generic_row_font_size(&mut self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let theme = cx.theme().clone();
        crate::ui::setting_row(
            "app-font-size",
            "Font size",
            "Sample control: editor font size for this application.",
            NumberInput::new(&self.font_size)
                .w(px(120.))
                .into_any_element(),
            Some(
                div()
                    .text_color(theme.muted_foreground)
                    .child("Mock value; not persisted in this phase."),
            )
            .into_any(),
            &theme,
        )
        .into_any_element()
    }

    fn render_opacity_row(&mut self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let theme = cx.theme().clone();
        let value = match self.opacity.read(cx).value() {
            SliderValue::Single(v) => v,
            SliderValue::Range(a, _) => a,
        };
        crate::ui::setting_row(
            "app-opacity",
            "Opacity",
            "Sample control: window opacity for this application.",
            h_flex()
                .w_full()
                .gap_2()
                .items_center()
                .child(Slider::new(&self.opacity).flex_1())
                .child(
                    div()
                        .w(px(44.))
                        .text_size(px(metrics::TEXT_BODY))
                        .text_color(theme.foreground)
                        .child(format!("{value:.0}%")),
                )
                .into_any_element(),
            None,
            &theme,
        )
        .into_any_element()
    }

    fn render_cursor_blink_row(&mut self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let theme = cx.theme().clone();
        let checked = self.cursor_blink;
        crate::ui::setting_row(
            "app-cursor-blink",
            "Cursor blink",
            "Sample control: whether the terminal cursor blinks.",
            Switch::new("app-cursor-blink-switch")
                .checked(checked)
                .on_click(cx.listener(|this, checked: &bool, _window, cx| {
                    this.cursor_blink = *checked;
                    cx.notify();
                }))
                .into_any_element(),
            None,
            &theme,
        )
        .into_any_element()
    }

    fn render_config_source_row(
        &mut self,
        source: String,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let theme = cx.theme().clone();
        crate::ui::setting_row(
            "app-config-source",
            "Configuration source",
            "The location this application's settings live in, as discovered. Licoman treats the source as authoritative.",
            Button::new("source-details")
                .label("Details…")
                .outline()
                .small()
                .on_click(cx.listener(move |_this, _: &gpui_kit::ClickEvent, window, cx| {
                    let source = source.clone();
                    window.open_dialog(cx, move |dialog, _window, _cx| {
                        dialog
                            .title("Configuration source")
                            .keyboard(true)
                            .child(
                                v_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .font_family("monospace")
                                            .text_size(px(metrics::TEXT_META))
                                            .child(source.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(metrics::TEXT_META))
                                            .text_color(_cx.theme().muted_foreground)
                                            .child(
                                                "Discovered location. Reading it arrives with \
                                                 the Phase 05 read backends.",
                                            ),
                                    ),
                            )
                    });
                }))
                .into_any_element(),
            None,
            &theme,
        )
        .into_any_element()
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

fn redacted_source(app: &licoman_core::discovery::InstalledApp) -> String {
    app.source_path.display().to_string()
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

impl AppsWindow {
    /// Visual-verification hook (`LICOMAN_DEMO=all-apps`): open the window
    /// directly in the "All installed" view.
    pub fn demo_show_all(&mut self) {
        self.show_all = true;
    }
}

impl Render for AppsWindow {
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl gpui_kit::IntoElement {
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
