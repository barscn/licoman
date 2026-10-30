//! Applications window: filterable list of installed applications plus a
//! detail pane with per-app setting categories (01_DESIGN_SYSTEM.md §4B).
//! Phase 01 shows mock applications, clearly labeled; unknown apps must
//! never appear as successfully configurable.

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
use licoman_core::mock::{APPS_CATALOG, Support};
use licoman_core::prefs::AppearanceMode;
use licoman_core::{Command, WindowRole};

use crate::actions::*;
use crate::app_state::{AppEvent, AppState, open_or_focus};
use crate::theme::metrics;
use crate::ui::{OptionIntoAny as _, settings_group, support_tag};

pub struct AppsWindow {
    app: Entity<AppState>,
    filter: Entity<InputState>,
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
            move |_this, _app, event: &AppEvent, cx| match event {
                AppEvent::PreferencesChanged | AppEvent::SettingsChanged => cx.notify(),
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
            font_size,
            opacity: opacity_for_event,
            cursor_blink: true,
            selected_tab: 0,
            _subscriptions: subscriptions,
            _appearance_sub: Some(appearance_sub),
        }
    }

    fn filtered_apps(&self, cx: &Context<Self>) -> Vec<usize> {
        let query = self.filter.read(cx).value().to_lowercase();
        APPS_CATALOG
            .iter()
            .enumerate()
            .filter(|(_, app)| query.is_empty() || app.name.to_lowercase().contains(&query))
            .map(|(i, _)| i)
            .collect()
    }

    fn render_list(&mut self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let theme = cx.theme().clone();
        let selected = self.app.read(cx).store.selected_app();
        let items: Vec<gpui_kit::AnyElement> = self
            .filtered_apps(cx)
            .into_iter()
            .map(|index| {
                let app = &APPS_CATALOG[index];
                let is_selected = selected == Some(index);
                let app_id = app.id;
                div()
                    .id(SharedString::from(format!("app-{index}")))
                    .px_3()
                    .py_2()
                    .rounded_sm()
                    .when(is_selected, |row| row.bg(theme.selection))
                    .hover(|row| row.bg(theme.accent))
                    .on_click(
                        cx.listener(move |this, _: &gpui_kit::ClickEvent, _window, cx| {
                            this.app.update(cx, |state, cx| {
                                state.dispatch(Command::SelectApp { index: Some(index) }, cx);
                                state.push_history(
                                    WindowRole::Applications,
                                    format!("app:{app_id}"),
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
                                    .child(app.name),
                            )
                            .child(support_tag(app.support)),
                    )
                    .child(
                        div()
                            .text_size(px(metrics::TEXT_META))
                            .text_color(theme.muted_foreground)
                            .child(app.description),
                    )
                    .into_any_element()
            })
            .collect();

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
            .child(
                Input::new(&self.filter)
                    .id("app-filter")
                    .small()
                    .cleanable(true),
            )
            .when(empty, |list| {
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
                                .child("Adjust the filter to see sample applications."),
                        ),
                )
            })
            .when(!empty, |list| list.children(items))
            .into_any_element()
    }

    fn render_detail(&mut self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let theme = cx.theme().clone();
        let selected = self.app.read(cx).store.selected_app();
        let Some(index) = selected.filter(|i| *i < APPS_CATALOG.len()) else {
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
        let app = &APPS_CATALOG[index];
        let tabs = app.categories;
        if self.selected_tab >= tabs.len() {
            self.selected_tab = 0;
        }

        // Tab bar (custom pills; gpui-kit tabs are heavier than needed here).
        let tab_bar = h_flex()
            .gap_1()
            .flex_wrap()
            .children(tabs.iter().enumerate().map(|(i, (name, _))| {
                let is_active = i == self.selected_tab;
                div()
                    .id(SharedString::from(format!("tab-{index}-{i}")))
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

        let header = v_flex()
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
                            .child(app.name),
                    )
                    .child(support_tag(app.support)),
            )
            .child(
                div()
                    .text_size(px(metrics::TEXT_BODY))
                    .text_color(theme.muted_foreground)
                    .child(app.description),
            );

        let body: gpui_kit::AnyElement = match app.support {
            Support::Unsupported => v_flex()
                .gap_2()
                .pt_4()
                .child(
                    div()
                        .text_size(px(metrics::TEXT_LABEL))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.foreground)
                        .child("No integrations for this application yet"),
                )
                .child(
                    div()
                        .text_size(px(metrics::TEXT_BODY))
                        .text_color(theme.muted_foreground)
                        .child(
                            "Licoman does not know any settings for this application, so it is shown as unsupported rather than pretending to configure it.",
                        ),
                )
                .into_any_element(),
            Support::Partial => {
                let tab_description = tabs
                    .get(self.selected_tab)
                    .map(|(_, d)| *d)
                    .unwrap_or("");
                let rows = vec![
                    self.render_generic_row_font_size(cx),
                    self.render_config_source_row(app.source, cx),
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
                let rows = vec![
                    self.render_generic_row_font_size(cx),
                    self.render_opacity_row(cx),
                    self.render_cursor_blink_row(cx),
                    self.render_config_source_row(app.source, cx),
                ];
                v_flex()
                    .gap_6()
                    .pt_4()
                    .child(settings_group(
                        tabs.get(self.selected_tab).map(|(n, _)| *n).unwrap_or("Settings"),
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
        source: &'static str,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let theme = cx.theme().clone();
        crate::ui::setting_row(
            "app-config-source",
            "Configuration source",
            "The file this application's settings are read from. Licoman treats it as authoritative.",
            Button::new("source-details")
                .label("Details…")
                .outline()
                .small()
                .on_click(cx.listener(move |_this, _: &gpui_kit::ClickEvent, window, cx| {
                    let source = source;
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
                                            .child(source),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(metrics::TEXT_META))
                                            .text_color(_cx.theme().muted_foreground)
                                            .child(
                                                "Sample path. Real source tracking arrives with the registry and read-only backends.",
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

impl Render for AppsWindow {
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        let theme = cx.theme().clone();
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
                                .child("Sample applications — discovery arrives in Phase 03."),
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
                    .child(self.render_list(cx))
                    .child(self.render_detail(cx)),
            )
    }
}
