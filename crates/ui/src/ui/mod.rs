//! Shared Licoman UI building blocks: thin wrappers over verified gpui-kit
//! primitives implementing the Quiet Control component anatomy
//! (01_DESIGN_SYSTEM.md §5). Presentational only — interactive controls are
//! wired in the window views, where view state is known.

pub mod apps_panel;
pub mod main_window;
pub mod preferences_panel;
pub mod setting_controls;
pub mod summary_panel;
pub mod user_panel;

use gpui_kit::component::tag::Tag;
use gpui_kit::component::theme::Theme;
use gpui_kit::component::{Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Div, FontWeight, InteractiveElement as _, ParentElement as _, SharedString,
    Stateful, Styled, div, px,
};

use crate::theme::metrics;
use licoman_core::modules::def::SupportLevel;

/// Scope/window heading: 24 px, weight 600 (typography table).
pub fn window_heading(text: impl Into<SharedString>, theme: &Theme) -> Div {
    div()
        .text_size(px(metrics::TEXT_WINDOW_HEADING))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.foreground)
        .child(text.into())
}

/// Group heading: 16 px semibold, hairline separator underneath.
pub fn group_heading(text: impl Into<SharedString>, theme: &Theme) -> Div {
    div().pb_2().border_b_1().border_color(theme.border).child(
        div()
            .text_size(px(metrics::TEXT_GROUP_HEADING))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.foreground)
            .child(text.into()),
    )
}

/// A settings group: heading, then rows; 24 px vertical group separation is
/// applied by the caller between groups.
pub fn settings_group(title: impl Into<SharedString>, rows: Vec<AnyElement>, theme: &Theme) -> Div {
    v_flex()
        .gap_1()
        .child(group_heading(title, theme))
        .children(rows)
}

/// One setting row: label + description left, control right-aligned in a
/// 200–260 px column, state/validation message exposed below — never
/// truncated beyond retrieval (§5). Wraps under the text when narrow.
pub fn setting_row(
    row_id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    description: impl Into<SharedString>,
    control: AnyElement,
    status_line: Option<AnyElement>,
    theme: &Theme,
) -> Stateful<Div> {
    v_flex()
        .id(row_id.into())
        .py_3()
        .min_h(px(metrics::SETTING_ROW_MIN_HEIGHT))
        .border_b_1()
        .border_color(theme.border)
        .child(
            h_flex()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_3()
                .child(
                    v_flex()
                        .min_w(px(220.))
                        .flex_1()
                        .gap_0p5()
                        .child(
                            div()
                                .text_size(px(metrics::TEXT_LABEL))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.foreground)
                                .child(label.into()),
                        )
                        .child(
                            div()
                                .text_size(px(metrics::TEXT_BODY))
                                .text_color(theme.muted_foreground)
                                .child(description.into()),
                        ),
                )
                .child(
                    h_flex()
                        .w(px(metrics::CONTROL_COLUMN_WIDTH))
                        .flex_shrink_0()
                        .justify_end()
                        .items_center()
                        .child(control),
                ),
        )
        .when_some(status_line, |row, line| {
            row.child(
                div()
                    .pt_1()
                    .w_full()
                    .text_size(px(metrics::TEXT_META))
                    .child(line),
            )
        })
}

/// Read-only value: stable text + explanation, never a disabled interactive
/// control pretending to be editable (§5 control types).
pub fn read_only_value(value: impl Into<SharedString>, theme: &Theme) -> Div {
    div()
        .text_size(px(metrics::TEXT_BODY))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.muted_foreground)
        .child(value.into())
}

/// Support pill for the Applications window list and detail header. The
/// labels are the plan's truthful levels (05_READ_ONLY_BACKENDS.md §5:
/// "Supported (read-only)" / "Partially supported").
pub fn support_tag(support: SupportLevel) -> Tag {
    match support {
        SupportLevel::Supported => Tag::success().small().child("Supported"),
        SupportLevel::Partial => Tag::warning().small().child("Partially supported"),
        SupportLevel::ReadOnly => Tag::info().small().child("Supported (read-only)"),
        SupportLevel::Unsupported => Tag::secondary().small().child("Unsupported"),
    }
}

/// `Some(element).into_any()` convenience for optional status lines.
pub trait OptionIntoAny {
    fn into_any(self) -> Option<AnyElement>;
}

impl<E: gpui_kit::IntoElement> OptionIntoAny for Option<E> {
    fn into_any(self) -> Option<AnyElement> {
        self.map(|element| element.into_any_element())
    }
}
