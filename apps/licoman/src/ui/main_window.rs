//! Main window: scope sidebar, settings content, search palette, apply bar.
//! Structure follows 01_DESIGN_SYSTEM.md §4A. All content is mock data and
//! labeled as such; states that need real providers render truthfully.

use gpui_kit::component::IndexPath;
use gpui_kit::component::TitleBar;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, NumberInput};
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent, SelectState};
use gpui_kit::component::sidebar::{Sidebar, SidebarMenu, SidebarMenuItem};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState, SliderValue};
use gpui_kit::component::switch::Switch;
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

use crate::actions::*;
use crate::app_state::{AppEvent, AppState, WindowRegistry, focus_window};
use crate::mock::{ControlKind, MockSetting, MockValue, RowStatus, Scope};
use crate::prefs::AppearanceMode;
use crate::theme::metrics;
use crate::ui::{OptionIntoAny as _, read_only_value, setting_row, settings_group, window_heading};

pub struct MainWindow {
    app: Entity<AppState>,
    header_search: Entity<InputState>,
    palette_input: Entity<InputState>,
    palette_open: bool,
    palette_selected: usize,
    volume_slider: Entity<SliderState>,
    dns_input: Entity<InputState>,
    downloads_input: Entity<InputState>,
    dropdowns: Vec<(String, Entity<SelectState<SearchableVec<&'static str>>>)>,
    select_resync_needed: bool,
    _subscriptions: Vec<Subscription>,
    _appearance_sub: Option<Subscription>,
}

impl MainWindow {
    pub fn new(app: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let header_search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search all settings… (Ctrl+K)"));
        let palette_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search all settings…"));

        // One slider per slider-kind mock setting; Phase 01 has one.
        let volume_slider = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(100.)
                .step(1.)
                .default_value(65.)
        });

        let dns_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Leave empty to keep system default")
        });
        let downloads_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("5")
                .step(1.)
                .min(0.)
                .max(64.)
                .default_value("5")
        });

        let mut dropdowns = Vec::new();
        let mut subscriptions = Vec::new();

        let dropdown_specs: Vec<(String, Vec<&'static str>, usize)> = app
            .read(cx)
            .settings
            .iter()
            .filter(|s| s.kind == ControlKind::Dropdown && !s.status.blocks_editing())
            .map(|setting| {
                let options: Vec<&'static str> = setting.options.to_vec();
                let current = match &setting.value {
                    MockValue::Option(o) => options
                        .iter()
                        .position(|opt| *opt == o.as_ref())
                        .unwrap_or(0),
                    _ => 0,
                };
                (setting.id.to_string(), options, current)
            })
            .collect();
        for (id, options, current) in dropdown_specs {
            let state = cx.new(|cx| {
                SelectState::new(
                    SearchableVec::new(options),
                    Some(IndexPath::new(current)),
                    window,
                    cx,
                )
            });
            dropdowns.push((id, state));
        }

        // Slider edits flow into the shared state; the control itself never
        // owns the committed value.
        let app_for_slider = app.clone();
        subscriptions.push(cx.subscribe(&volume_slider, {
            move |_this, _slider, event: &SliderEvent, cx| {
                if let SliderEvent::Change(SliderValue::Single(v)) = event {
                    app_for_slider.update(cx, |state, cx| {
                        state.edit(
                            "shared.audio.output_volume",
                            MockValue::Number(*v as f64),
                            cx,
                        );
                    });
                }
            }
        }));

        let app_for_dns = app.clone();
        subscriptions.push(cx.subscribe(&dns_input, {
            move |_this, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    let value = input.read(cx).value().to_string();
                    app_for_dns.update(cx, |state, cx| {
                        state.edit("shared.network.dns", MockValue::Text(value.into()), cx);
                    });
                }
            }
        }));

        let app_for_downloads = app.clone();
        subscriptions.push(cx.subscribe(&downloads_input, {
            move |_this, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    let text = input.read(cx).value().to_string();
                    if let Ok(n) = text.trim().parse::<f64>() {
                        app_for_downloads.update(cx, |state, cx| {
                            state.edit(
                                "distro.packages.parallel_downloads",
                                MockValue::Number(n),
                                cx,
                            );
                        });
                    }
                }
            }
        }));

        // Select confirmations flow into the shared state.
        for (id, state) in &dropdowns {
            let id = id.clone();
            let app_for_select = app.clone();
            subscriptions.push(cx.subscribe(state, {
                move |_this, _select, event: &SelectEvent<SearchableVec<&'static str>>, cx| {
                    if let SelectEvent::Confirm(Some(value)) = event {
                        let value = (*value).to_string();
                        let id = id.clone();
                        app_for_select.update(cx, |state, cx| {
                            state.edit(&id, MockValue::Option(value.into()), cx);
                        });
                    }
                }
            }));
        }

        // Track app events: resync owned controls when the store changes
        // (discard, apply) so controls never present a stale value.
        subscriptions.push(cx.subscribe(&app, {
            move |this, _app, event: &AppEvent, cx| match event {
                AppEvent::SettingsChanged => {
                    this.select_resync_needed = true;
                    cx.notify();
                }
                AppEvent::PreferencesChanged => cx.notify(),
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
                if state.preferences.theme == AppearanceMode::System {
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
            volume_slider,
            dns_input,
            downloads_input,
            dropdowns,
            select_resync_needed: false,
            _subscriptions: subscriptions,
            _appearance_sub: Some(appearance_sub),
        };
        this.sync_owned_controls(window, cx);
        this
    }

    /// Push store values into the controls that keep local display state
    /// (Select, Slider, Input) so the UI never shows a stale value after
    /// discard/apply.
    fn sync_owned_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.select_resync_needed = false;

        for (id, select) in &self.dropdowns {
            let current: Option<&'static str> = {
                let app = self.app.read(cx);
                app.setting(id).and_then(|s| match &s.value {
                    MockValue::Option(o) => {
                        s.options.iter().find(|opt| **opt == o.as_ref()).copied()
                    }
                    _ => None,
                })
            };
            if let Some(current) = current {
                select.update(cx, |state, cx| {
                    state.set_selected_value(&current, window, cx);
                });
            }
        }

        let volume = self
            .app
            .read(cx)
            .setting("shared.audio.output_volume")
            .and_then(|s| match s.value {
                MockValue::Number(n) => Some(n as f32),
                _ => None,
            });
        if let Some(v) = volume {
            self.volume_slider.update(cx, |s, cx| {
                s.set_value(v, window, cx);
            });
        }

        let downloads = self
            .app
            .read(cx)
            .setting("distro.packages.parallel_downloads")
            .and_then(|s| match s.value {
                MockValue::Number(n) => Some(format!("{n}")),
                _ => None,
            });
        if let Some(text) = downloads
            && self.downloads_input.read(cx).value() != text.as_str()
        {
            self.downloads_input.update(cx, |s, cx| {
                s.set_value(text.clone(), window, cx);
            });
        }

        let dns = self
            .app
            .read(cx)
            .setting("shared.network.dns")
            .and_then(|s| match &s.value {
                MockValue::Text(t) => Some(t.to_string()),
                _ => None,
            });
        if let Some(text) = dns
            && self.dns_input.read(cx).value() != text.as_str()
        {
            self.dns_input.update(cx, |s, cx| {
                s.set_value(text.clone(), window, cx);
            });
        }
    }

    /// Visual-verification hook (`LICOMAN_DEMO=palette`): opens the palette
    /// without pointer/keyboard interaction.
    pub fn demo_open_palette(&mut self) {
        self.palette_open = true;
        self.palette_selected = 0;
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

    fn palette_results(&self, cx: &Context<Self>) -> Vec<usize> {
        let query = self.palette_input.read(cx).value().to_string();
        self.app.read(cx).search(&query)
    }

    fn accept_palette_result(&mut self, cx: &mut Context<Self>) {
        let results = self.palette_results(cx);
        let picked = results
            .get(self.palette_selected.min(results.len().saturating_sub(1)))
            .copied();
        if let Some(index) = picked {
            let scope = self.app.read(cx).settings[index].scope;
            self.app.update(cx, |state, _cx| {
                state.selected_scope = scope;
            });
        }
        self.palette_open = false;
        cx.notify();
    }

    // ---- row rendering ----------------------------------------------------

    fn render_setting(
        &mut self,
        index: usize,
        setting: &MockSetting,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let control = self.render_control(setting, window, cx);
        let status = self.render_status(setting, cx);
        let _ = window;
        setting_row(
            SharedString::from(format!("setting-row-{index}")),
            setting.label,
            setting.description,
            control,
            status,
            &theme,
        )
        .into_any_element()
    }

    fn render_control(
        &mut self,
        setting: &MockSetting,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let id = setting.id.to_string();
        match setting.kind {
            ControlKind::Switch => {
                let checked = matches!(setting.value, MockValue::Bool(true));
                Switch::new(setting.id.clone())
                    .checked(checked)
                    .disabled(setting.status.blocks_editing())
                    .on_click(cx.listener(move |this, checked: &bool, _window, cx| {
                        let value = MockValue::Bool(*checked);
                        this.app
                            .update(cx, |state, cx| state.edit(&id.clone(), value, cx));
                    }))
                    .into_any_element()
            }
            ControlKind::Dropdown => {
                if setting.status.blocks_editing() {
                    return read_only_value(setting.value.as_text(), &theme).into_any_element();
                }
                let state = self
                    .dropdowns
                    .iter()
                    .find(|(dropdown_id, _)| *dropdown_id == id)
                    .map(|(_, state)| state.clone());
                match state {
                    Some(state) => Select::new(&state)
                        .id(setting.id.clone())
                        .menu_width(px(200.))
                        .into_any_element(),
                    None => read_only_value(setting.value.as_text(), &theme).into_any_element(),
                }
            }
            ControlKind::Slider => {
                if setting.status.blocks_editing() {
                    return read_only_value(
                        format!("{}{}", mock_number(setting), setting.unit),
                        &theme,
                    )
                    .into_any_element();
                }
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .child(Slider::new(&self.volume_slider).flex_1())
                    .child(
                        div()
                            .w(px(44.))
                            .text_size(px(metrics::TEXT_BODY))
                            .text_color(theme.foreground)
                            .child(format!("{}{}", mock_number(setting), setting.unit)),
                    )
                    .into_any_element()
            }
            ControlKind::Number => {
                if setting.status.blocks_editing() {
                    return read_only_value(mock_number(setting).to_string(), &theme)
                        .into_any_element();
                }
                NumberInput::new(&self.downloads_input)
                    .w(px(120.))
                    .into_any_element()
            }
            ControlKind::Text => {
                if setting.status.blocks_editing() {
                    return read_only_value(
                        if setting.value.as_text().is_empty() {
                            "Not set".to_string()
                        } else {
                            setting.value.as_text().to_string()
                        },
                        &theme,
                    )
                    .into_any_element();
                }
                Input::new(&self.dns_input)
                    .id(setting.id.clone())
                    .w(px(220.))
                    .into_any_element()
            }
            ControlKind::ReadOnly => {
                read_only_value(setting.value.as_text(), &theme).into_any_element()
            }
        }
    }

    fn render_status(
        &mut self,
        setting: &MockSetting,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let theme = cx.theme().clone();
        let id = setting.id.to_string();
        match &setting.status {
            RowStatus::Idle => None,
            RowStatus::Dirty => Some(
                h_flex()
                    .gap_1()
                    .items_center()
                    .text_color(theme.warning)
                    .child(Icon::new(IconName::CircleAlert).small())
                    .child("Unsaved change"),
            )
            .into_any(),
            RowStatus::Applying => Some(
                h_flex()
                    .gap_1()
                    .items_center()
                    .text_color(theme.muted_foreground)
                    .child("Applying…"),
            )
            .into_any(),
            RowStatus::Saved => Some(
                h_flex()
                    .gap_1()
                    .items_center()
                    .text_color(theme.success)
                    .child(Icon::new(IconName::CircleCheck).small())
                    .child("Saved"),
            )
            .into_any(),
            RowStatus::Failed { reason } => {
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
                                .child(format!("Failed — {reason}")),
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
                                                    state.retry_failed(&setting_id, cx)
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
                                                    state.revert_failed(&setting_id, cx)
                                                });
                                            },
                                        )),
                                ),
                        ),
                )
                .into_any()
            }
            RowStatus::ExternalChange => {
                let reload_id = SharedString::from(format!("reload-{id}"));
                let compare_id = SharedString::from(format!("compare-{id}"));
                let reload_setting_id = id.clone();
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
                                                    state.acknowledge_external_change(
                                                        &setting_id, cx,
                                                    )
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
                                            |_this, _: &ClickEvent, window, cx| {
                                                window.open_dialog(cx, |dialog, _window, cx| {
                                                dialog
                                                    .title("Compare changes")
                                                    .keyboard(true)
                                                    .child(
                                                        v_flex()
                                                            .gap_2()
                                                            .child(div().child(
                                                                "Licoman (sample): accent = #70D0A9",
                                                            ))
                                                            .child(div().child(
                                                                "Desktop (current): accent = #8AB4FF",
                                                            ))
                                                            .child(
                                                                div()
                                                                    .text_color(cx.theme().muted_foreground)
                                                                    .text_size(px(metrics::TEXT_META))
                                                                    .child(
                                                                        "Sample comparison; the desktop value remains authoritative until reloaded.",
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
            RowStatus::PermissionNeeded { scope } => Some(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .text_color(theme.muted_foreground)
                            .child(Icon::new(gpui_kit::assets::IconName::Lock).small())
                            .child(format!("Requires permission — {scope}")),
                    )
                    .child(
                        h_flex().gap_2().child(
                            Button::new(SharedString::from(format!("authorize-{id}")))
                                .label("Authorize…")
                                .outline()
                                .xsmall()
                                .on_click(cx.listener(|_this, _: &ClickEvent, window, cx| {
                                    window.open_alert_dialog(cx, |alert, _window, _cx| {
                                        alert
                                            .title("Authorization is not available in this build")
                                            .description(
                                                "Licoman runs unprivileged. Privileged edits are deferred until the safe write engine and its authorization flow are implemented and reviewed (Phases 06–07). No system configuration was modified.",
                                            )
                                    });
                                })),
                        ),
                    ),
            )
            .into_any(),
            RowStatus::RestartNeeded { target } => Some(
                h_flex()
                    .gap_1()
                    .items_center()
                    .text_color(theme.warning)
                    .child(Icon::new(IconName::TriangleAlert).small())
                    .child(format!("Applies after restarting {target}")),
            )
            .into_any(),
            RowStatus::Unsupported { reason } => Some(
                div()
                    .text_color(theme.muted_foreground)
                    .child(reason.clone()),
            )
            .into_any(),
        }
    }

    // ---- layout -----------------------------------------------------------

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let selected = self.app.read(cx).selected_scope;

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

        let environment = v_flex()
            .px_3()
            .pb_3()
            .gap_1()
            .child(
                div()
                    .text_size(px(metrics::TEXT_META))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.muted_foreground)
                    .child("Environment"),
            )
            .children(self.app.read(cx).environment().iter().map(|(name, _kind)| {
                div()
                    .text_size(px(metrics::TEXT_BODY))
                    .text_color(theme.foreground)
                    .child(*name)
            }))
            .child(
                div()
                    .text_size(px(metrics::TEXT_META))
                    .text_color(theme.muted_foreground)
                    .child("Sample data — real discovery arrives in Phase 03."),
            );

        Sidebar::new("scope-sidebar")
            .w(px(metrics::SIDEBAR_WIDTH))
            .collapsible(false)
            .child(menu)
            .footer(environment)
            .into_any_element()
    }

    fn render_content(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let scope = self.app.read(cx).selected_scope;
        let settings: Vec<MockSetting> = self
            .app
            .read(cx)
            .settings
            .iter()
            .filter(|s| s.scope == scope)
            .cloned()
            .collect();

        let mut categories: Vec<&'static str> = Vec::new();
        for setting in &settings {
            if !categories.contains(&setting.category) {
                categories.push(setting.category);
            }
        }

        let mut groups: Vec<AnyElement> = Vec::new();
        for category in categories {
            let mut rows: Vec<AnyElement> = Vec::new();
            for (i, s) in settings.iter().enumerate() {
                if s.category == category {
                    rows.push(self.render_setting(i, s, window, cx));
                }
            }
            groups.push(settings_group(category, rows, &theme).into_any_element());
        }

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
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                gpui_kit::component::tag::Tag::secondary()
                                    .small()
                                    .child("Mock data"),
                            )
                            .child(
                                div()
                                    .text_size(px(metrics::TEXT_META))
                                    .text_color(theme.muted_foreground)
                                    .child("All values below are samples for design review."),
                            ),
                    )
                    .children(groups),
            )
            .into_any_element()
    }

    fn render_apply_bar(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme().clone();
        let dirty = self.app.read(cx).dirty_count();
        let applying = self.app.read(cx).applying;
        if dirty == 0 && !applying {
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
                                        "{dirty} change{} pending",
                                        if dirty == 1 { "" } else { "s" }
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
                                    this.app.update(cx, |state, cx| state.discard_all(cx));
                                })),
                        )
                        .child(
                            Button::new("apply-changes")
                                .label(if applying { "Applying…" } else { "Apply" })
                                .primary()
                                .loading(applying)
                                .disabled(applying)
                                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                                    this.app.update(cx, |state, cx| state.apply_pending(cx));
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
            .map(|(i, &index)| {
                let setting = &self.app.read(cx).settings[index];
                let label = setting.label.to_string();
                let breadcrumb = crate::mock::breadcrumb(setting);
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
                                div()
                                    .text_size(px(metrics::TEXT_LABEL))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.foreground)
                                    .child(label),
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
                                                "Type to search across every sample setting; results show scope → category → setting."
                                            } else {
                                                "Try a different search. Sample settings cover Shared, Desktop and Distribution."
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
        let existing = cx.global::<WindowRegistry>().apps_window;
        if focus_window(&existing, cx) {
            return;
        }
        let app = self.app.clone();
        let handle = crate::windows::open_apps_window(&app, cx);
        cx.global_mut::<WindowRegistry>().apps_window = Some(handle);
    }

    fn open_preferences_window(&mut self, cx: &mut Context<Self>) {
        let existing = cx.global::<WindowRegistry>().preferences_window;
        if focus_window(&existing, cx) {
            return;
        }
        let app = self.app.clone();
        let handle = crate::windows::open_preferences_window(&app, cx);
        cx.global_mut::<WindowRegistry>().preferences_window = Some(handle);
    }
}

fn mock_number(setting: &MockSetting) -> f64 {
    match setting.value {
        MockValue::Number(n) => n,
        _ => 0.,
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
            this.app.update(cx, |state, _cx| {
                state.selected_scope = scope;
            });
        }))
}

impl Render for MainWindow {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        if self.select_resync_needed {
            self.sync_owned_controls(window, cx);
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
