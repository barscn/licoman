//! Licoman Preferences — product-only preferences (application theme), a
//! dedicated native window (01_DESIGN_SYSTEM.md §4C). Never synchronized with
//! GNOME/KDE/system appearance settings.

use gpui_kit::component::TitleBar;
use gpui_kit::component::radio::{Radio, RadioGroup};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::theme::ThemeMode;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Context, Entity, FontWeight, InteractiveElement as _, ParentElement as _, Render, SharedString,
    Styled, Window, div, px,
};
use licoman_core::prefs::AppearanceMode;

use crate::app_state::{AppEvent, AppState};
use crate::theme::metrics;

pub struct PreferencesWindow {
    app: Entity<AppState>,
    _appearance_sub: Option<gpui_kit::Subscription>,
    _prefs_sub: Option<gpui_kit::Subscription>,
}

impl PreferencesWindow {
    pub fn new(app: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let appearance_app = app.clone();
        let appearance_sub = window.observe_window_appearance(move |_window, cx| {
            appearance_app.update(cx, |state, cx| {
                if state.store.preferences().theme == AppearanceMode::System {
                    state.resolve_and_apply_theme(cx);
                }
            });
        });

        // Redraw on preference changes so the radio selection follows the store.
        let prefs_sub = cx.subscribe(&app, move |_this, _app, event: &AppEvent, cx| {
            if matches!(event, AppEvent::PreferencesChanged) {
                cx.notify();
            }
        });

        Self {
            app,
            _appearance_sub: Some(appearance_sub),
            _prefs_sub: Some(prefs_sub),
        }
    }
}

impl Render for PreferencesWindow {
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        let theme = cx.theme().clone();
        let prefs = self.app.read(cx).store.preferences().clone();
        let diagnostics = self.app.read(cx).store.preference_diagnostics().to_vec();
        let selected = AppearanceMode::ALL
            .iter()
            .position(|m| *m == prefs.theme)
            .unwrap_or(0);
        let using_system = prefs.theme == AppearanceMode::System;
        let effective = if using_system {
            Some(cx.window_appearance())
        } else {
            None
        };
        let system_disclosure = effective.map(|appearance| {
            let mode = crate::app_state::system_theme_mode(appearance);
            let resolved = if mode == ThemeMode::Dark { "dark" } else { "light" };
            format!(
                "System: following the desktop signal ({resolved}). If it becomes unavailable, Licoman uses Dark."
            )
        });

        v_flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                TitleBar::new()
                    .h(px(metrics::HEADER_HEIGHT))
                    .child(
                        h_flex()
                            .w_full()
                            .h_full()
                            .items_center()
                            .px_2()
                            .child(
                                div()
                                    .text_size(px(metrics::TEXT_LABEL))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Licoman Preferences"),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .id("preferences-content")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .px(px(metrics::CONTENT_PADDING))
                    .py_5()
                    .gap_6()
                    .child(
                        v_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(metrics::TEXT_GROUP_HEADING))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.foreground)
                                    .child("Licoman appearance"),
                            )
                            .child(
                                div()
                                    .text_size(px(metrics::TEXT_BODY))
                                    .text_color(theme.muted_foreground)
                                    .child(
                                        "Applies to Licoman only. Your desktop environment's own theme settings are never changed.",
                                    ),
                            )
                            .child(
                                RadioGroup::horizontal("appearance-theme")
                                    .selected_index(Some(selected))
                                    .on_click(cx.listener(|this, index: &usize, _window, cx| {
                                        let mode = AppearanceMode::ALL
                                            .get(*index)
                                            .copied()
                                            .unwrap_or(AppearanceMode::Dark);
                                        this.app.update(cx, |state, cx| {
                                            state.change_theme(mode, cx);
                                        });
                                    }))
                                    .children(AppearanceMode::ALL.map(|mode| {
                                        Radio::new(SharedString::from(format!(
                                            "theme-{mode:?}"
                                        )))
                                        .label(mode.label())
                                    })),
                            )
                            .when(using_system, |section| {
                                section.child(
                                    h_flex()
                                        .gap_1()
                                        .items_center()
                                        .text_color(theme.muted_foreground)
                                        .child(
                                            Icon::new(IconName::Info)
                                                .small()
                                                .text_color(theme.info),
                                        )
                                        .child(match system_disclosure {
                                            Some(text) => text,
                                            None => {
                                                "System appearance unavailable; using Dark."
                                                    .to_string()
                                            }
                                        })
                                )
                            })
                            .child(
                                div()
                                    .text_size(px(metrics::TEXT_META))
                                    .text_color(theme.muted_foreground)
                                    .child(
                                        "The choice persists in Licoman's own preferences file and applies to every Licoman window immediately.",
                                    ),
                            ),
                    )
                    .when(!diagnostics.is_empty(), |root| {
                        root.child(
                            v_flex()
                                .gap_1()
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .child(
                                            Icon::new(IconName::TriangleAlert)
                                                .small()
                                                .text_color(theme.warning),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(metrics::TEXT_LABEL))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(theme.foreground)
                                                .child("Preferences file notices"),
                                        ),
                                )
                                .children(diagnostics.iter().map(|d| {
                                    div()
                                        .text_size(px(metrics::TEXT_BODY))
                                        .text_color(theme.muted_foreground)
                                        .child(d.message())
                                })),
                        )
                    })
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(metrics::TEXT_GROUP_HEADING))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.foreground)
                                    .child("About"),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_size(px(metrics::TEXT_BODY))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(theme.foreground)
                                            .child("Licoman"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(metrics::TEXT_BODY))
                                            .text_color(theme.muted_foreground)
                                            .child(env!("CARGO_PKG_VERSION")),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(metrics::TEXT_BODY))
                                    .text_color(theme.muted_foreground)
                                    .child("A universal interface for Linux configuration."),
                            )
                            .child(
                                div()
                                    .text_size(px(metrics::TEXT_META))
                                    .text_color(theme.muted_foreground)
                                    .child(
                                        "Existing configuration remains authoritative; Licoman never hides your files from you.",
                                    ),
                            ),
                    ),
            )
    }
}
