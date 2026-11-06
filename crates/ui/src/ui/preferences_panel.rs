//! Settings panel — product-only preferences (application theme), file
//! diagnostics and About, rendered inside the main window's content area
//! (single-window IA; 01_DESIGN_SYSTEM.md §4C). Never synchronized with
//! GNOME/KDE/system appearance settings.
//!
//! A plain render function, like [`crate::ui::summary_panel`]: the panel is
//! stateless (the store holds the selection) and its only interaction — the
//! appearance radio group — takes a plain closure over the shared
//! `Entity<AppState>`.

use gpui_kit::component::radio::{Radio, RadioGroup};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::theme::ThemeMode;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Entity, FontWeight, InteractiveElement as _, IntoElement as _,
    ParentElement as _, SharedString, Styled, div, px,
};
use licoman_core::prefs::AppearanceMode;

use crate::app_state::{AppState, system_theme_mode};
use crate::theme::metrics;
use crate::ui::window_heading;

pub fn render(app: &Entity<AppState>, cx: &App) -> AnyElement {
    let theme = cx.theme().clone();
    let prefs = app.read(cx).store.preferences().clone();
    let diagnostics = app.read(cx).store.preference_diagnostics().to_vec();
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
        let mode = system_theme_mode(appearance);
        let resolved = if mode == ThemeMode::Dark { "dark" } else { "light" };
        format!(
            "System: following the desktop signal ({resolved}). If it becomes unavailable, Licoman uses Dark."
        )
    });
    let radio_app = app.clone();

    v_flex()
        .id("preferences-content")
        .size_full()
        .overflow_y_scrollbar()
        .px(px(metrics::CONTENT_PADDING))
        .py_5()
        .gap_4()
        .child(window_heading("Settings", &theme))
        .child(
            v_flex()
                .max_w(px(metrics::CONTENT_MAX_WIDTH))
                .gap_2()
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
                        .on_click(move |index: &usize, _window, cx: &mut App| {
                            let mode = AppearanceMode::ALL
                                .get(*index)
                                .copied()
                                .unwrap_or(AppearanceMode::Dark);
                            radio_app.update(cx, |state, cx| {
                                state.change_theme(mode, cx);
                            });
                        })
                        .children(AppearanceMode::ALL.map(|mode| {
                            Radio::new(SharedString::from(format!("theme-{mode:?}")))
                                .label(mode.label())
                        })),
                )
                .when(using_system, |section| {
                    section.child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .text_color(theme.muted_foreground)
                            .child(Icon::new(IconName::Info).small().text_color(theme.info))
                            .child(match system_disclosure {
                                Some(text) => text,
                                None => {
                                    "System appearance unavailable; using Dark.".to_string()
                                }
                            }),
                    )
                })
                .child(
                    div()
                        .text_size(px(metrics::TEXT_META))
                        .text_color(theme.muted_foreground)
                        .child(
                            "The choice persists in Licoman's own preferences file and applies immediately.",
                        ),
                ),
        )
        .when(!diagnostics.is_empty(), |root| {
            root.child(
                v_flex()
                    .max_w(px(metrics::CONTENT_MAX_WIDTH))
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
                .max_w(px(metrics::CONTENT_MAX_WIDTH))
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
        )
        .into_any_element()
}
