//! The **Quiet Control** design system — Licoman's semantic design tokens
//! (01_DESIGN_SYSTEM.md §3) mapped onto gpui-kit's theme API.
//!
//! Raw hex codes live only here. Views read tokens through `cx.theme()` (the
//! gpui-component theme, which this module fully overrides for both modes) or
//! through [`palette`] when a token has no gpui counterpart.

use gpui_kit::SharedString;
use gpui_kit::component::theme::{Theme, ThemeConfig, ThemeMode};

pub const THEME_DARK_NAME: &str = "Licoman Dark";
pub const THEME_LIGHT_NAME: &str = "Licoman Light";

/// Layout constants from the design spec (logical px).
/// Some constants document the full scale though Phase 01 uses a subset.
#[allow(dead_code)]
pub mod metrics {
    use gpui_kit::px;

    /// Spacing scale: 0, 2, 4, 8, 12, 16, 20, 24, 32, 40, 48.
    pub const SPACING: &[f32] = &[0., 2., 4., 8., 12., 16., 20., 24., 32., 40., 48.];
    pub const RADIUS_SM: f32 = 5.;
    pub const RADIUS_MD: f32 = 8.;
    pub const RADIUS_LG: f32 = 12.;
    pub const RADIUS_XL: f32 = 16.;

    pub const HEADER_HEIGHT: f32 = 56.;
    pub const SIDEBAR_WIDTH: f32 = 244.;
    pub const APPS_LIST_WIDTH: f32 = 260.;
    pub const CONTENT_MAX_WIDTH: f32 = 800.;
    pub const CONTENT_PADDING: f32 = 32.;
    pub const CONTENT_PADDING_NARROW: f32 = 20.;
    pub const GROUP_GAP: f32 = 24.;
    pub const SETTING_ROW_MIN_HEIGHT: f32 = 60.;
    pub const CONTROL_MIN_HEIGHT: f32 = 32.;
    pub const CONTROL_COLUMN_WIDTH: f32 = 220.;

    // Typography (logical px): (size, weight-ish role is applied by callers)
    pub const TEXT_WINDOW_HEADING: f32 = 24.;
    pub const TEXT_PAGE_HEADING: f32 = 21.;
    pub const TEXT_GROUP_HEADING: f32 = 16.;
    pub const TEXT_LABEL: f32 = 14.;
    pub const TEXT_BODY: f32 = 13.;
    pub const TEXT_META: f32 = 12.;

    // Window geometry — Licoman is a single-window app; Applications and
    // Settings render inside this window's content area.
    pub const MAIN_WINDOW_SIZE: gpui_kit::Size<gpui_kit::Pixels> = gpui_kit::Size {
        width: px(1160.),
        height: px(760.),
    };
    pub const MAIN_WINDOW_MIN: gpui_kit::Size<gpui_kit::Pixels> = gpui_kit::Size {
        width: px(760.),
        height: px(560.),
    };
}

/// Licoman's semantic color tokens. `text_primary` etc. use snake_case to stay
/// close to the token names in the design spec.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LicomanPalette {
    pub canvas: u32,
    pub sidebar: u32,
    pub surface: u32,
    pub raised: u32,
    pub hover: u32,
    pub selected: u32,
    pub border: u32,
    pub text_primary: u32,
    pub text_secondary: u32,
    pub text_muted: u32,
    pub accent: u32,
    pub on_accent: u32,
    pub success: u32,
    pub warning: u32,
    pub danger: u32,
    // Derived tones (not in the spec table; needed by toolkit controls).
    pub accent_hover: u32,
    pub accent_active: u32,
    pub info: u32,
    pub scrollbar_thumb: u32,
}

/// Curated Licoman dark palette — the default on first launch.
pub const DARK: LicomanPalette = LicomanPalette {
    canvas: 0x0E1117,
    sidebar: 0x141922,
    surface: 0x171D27,
    raised: 0x1D2633,
    hover: 0x253143,
    selected: 0x263753,
    border: 0x344155,
    text_primary: 0xEDF2F8,
    text_secondary: 0xB0BDCE,
    text_muted: 0x97A6B9,
    accent: 0x8AB4FF,
    on_accent: 0x102035,
    success: 0x70D0A9,
    warning: 0xF2C36C,
    danger: 0xFF9399,
    accent_hover: 0x9FC0FF,
    accent_active: 0x7BA9F2,
    info: 0x67B5E5,
    scrollbar_thumb: 0x2A3648,
};

/// Curated Licoman light palette — user selectable.
pub const LIGHT: LicomanPalette = LicomanPalette {
    canvas: 0xF4F6FA,
    sidebar: 0xEBEFF5,
    surface: 0xFFFFFF,
    raised: 0xF9FAFD,
    hover: 0xE7EDF7,
    selected: 0xE2ECFC,
    border: 0xD8E0EA,
    text_primary: 0x1A2635,
    text_secondary: 0x4D5E73,
    text_muted: 0x5A6B81,
    accent: 0x255FD4,
    on_accent: 0xFFFFFF,
    success: 0x167C5C,
    warning: 0x945A0B,
    danger: 0xB83243,
    accent_hover: 0x1E51BC,
    accent_active: 0x1A47A8,
    info: 0x1D6FB8,
    scrollbar_thumb: 0xC4CEDA,
};

/// The palette matching the current gpui theme mode. Reserved for wrappers
/// that need a token without a gpui counterpart.
#[allow(dead_code)]
pub fn palette(theme: &Theme) -> &'static LicomanPalette {
    if theme.is_dark() { &DARK } else { &LIGHT }
}

fn hex(c: u32) -> SharedString {
    format!("#{c:06X}").into()
}

/// Build the complete Licoman theme for `mode` as a gpui-kit `ThemeConfig`.
///
/// Every visually load-bearing color is set explicitly: `Theme::apply_config`
/// merges over the toolkit defaults, and anything left unset would leak the
/// stock shadcn palette into Licoman's surface.
pub fn theme_config(mode: ThemeMode) -> ThemeConfig {
    let dark = mode.is_dark();
    let p = if dark { &DARK } else { &LIGHT };
    let name = if dark {
        THEME_DARK_NAME
    } else {
        THEME_LIGHT_NAME
    };

    let fg_on_tint = hex(p.on_accent);
    let text_primary = hex(p.text_primary);
    let text_secondary = hex(p.text_secondary);
    let border = hex(p.border);
    let surface = hex(p.surface);
    let raised = hex(p.raised);
    let hover = hex(p.hover);
    let accent = hex(p.accent);

    let mut colors = gpui_kit::component::theme::ThemeConfigColors::default();
    colors.background = Some(hex(p.canvas));
    colors.foreground = Some(text_primary.clone());
    colors.border = Some(border.clone());
    colors.selection = Some(hex(p.selected));
    colors.ring = Some(accent.clone());
    colors.caret = Some(accent.clone());
    colors.link = Some(accent.clone());
    colors.drag_border = Some(accent.clone());
    colors.window_border = Some(border.clone());
    colors.overlay = Some(if dark {
        "#060810C0".into()
    } else {
        "#1A263540".into()
    });
    colors.progress_bar = Some(accent.clone());
    colors.skeleton = Some(raised.clone());
    colors.accordion = Some(surface.clone());
    colors.popover = Some(surface.clone());
    colors.popover_foreground = Some(text_primary.clone());
    colors.secondary = Some(raised.clone());
    colors.secondary_foreground = Some(text_primary.clone());
    colors.secondary_hover = Some(hover.clone());
    colors.secondary_active = Some(hover.clone());
    colors.muted = Some(raised.clone());
    colors.muted_foreground = Some(text_secondary.clone());
    colors.accent = Some(hover.clone());
    colors.accent_foreground = Some(text_primary.clone());
    colors.primary = Some(accent.clone());
    colors.primary_foreground = Some(fg_on_tint.clone());
    colors.primary_hover = Some(hex(p.accent_hover));
    colors.primary_active = Some(hex(p.accent_active));
    colors.input = Some(border.clone());
    colors.sidebar = Some(hex(p.sidebar));
    colors.sidebar_foreground = Some(text_secondary.clone());
    colors.sidebar_accent = Some(hover.clone());
    colors.sidebar_accent_foreground = Some(text_primary.clone());
    colors.sidebar_border = Some(border.clone());
    colors.sidebar_primary = Some(accent.clone());
    colors.sidebar_primary_foreground = Some(fg_on_tint.clone());
    colors.title_bar = Some(hex(p.sidebar));
    colors.title_bar_border = Some(border.clone());
    colors.status_bar = Some(surface.clone());
    colors.status_bar_border = Some(border.clone());
    colors.tab_bar = Some(hex(p.canvas));
    colors.tab_bar_segmented = Some(hex(p.sidebar));
    colors.tab = Some(surface.clone());
    colors.tab_active = Some(raised.clone());
    colors.tab_foreground = Some(text_secondary.clone());
    colors.tab_active_foreground = Some(text_primary.clone());
    colors.list = Some(surface.clone());
    colors.list_hover = Some(hover.clone());
    colors.list_active = Some(hex(p.selected));
    colors.list_active_border = Some(accent.clone());
    colors.list_even = Some(surface.clone());
    colors.list_head = Some(raised.clone());
    colors.table = Some(surface.clone());
    colors.table_hover = Some(hover.clone());
    colors.table_active = Some(hex(p.selected));
    colors.table_active_border = Some(accent.clone());
    colors.table_even = Some(surface.clone());
    colors.table_head = Some(raised.clone());
    colors.table_head_foreground = Some(text_secondary.clone());
    colors.table_row_border = Some(border.clone());
    colors.group_box = Some(surface.clone());
    colors.group_box_foreground = Some(text_primary.clone());
    colors.group_box_title_foreground = Some(text_secondary.clone());
    colors.description_list_label_foreground = Some(text_secondary.clone());
    colors.switch = Some(accent.clone());
    colors.switch_thumb = Some(fg_on_tint.clone());
    colors.slider_bar = Some(border.clone());
    colors.slider_thumb = Some(accent.clone());
    colors.button = Some(raised.clone());
    colors.button_foreground = Some(text_primary.clone());
    colors.button_hover = Some(hover.clone());
    colors.button_active = Some(hover.clone());
    colors.button_primary = Some(accent.clone());
    colors.button_primary_foreground = Some(fg_on_tint.clone());
    colors.button_primary_hover = Some(hex(p.accent_hover));
    colors.button_primary_active = Some(hex(p.accent_active));
    colors.button_secondary = Some(raised.clone());
    colors.button_secondary_foreground = Some(text_primary.clone());
    colors.button_secondary_hover = Some(hover.clone());
    colors.button_secondary_active = Some(hover.clone());
    colors.button_danger = Some(hex(p.danger));
    colors.button_danger_foreground = Some(fg_on_tint.clone());
    colors.button_danger_hover = Some(hex(p.danger));
    colors.button_danger_active = Some(hex(p.danger));
    colors.button_success = Some(hex(p.success));
    colors.button_success_foreground = Some(fg_on_tint.clone());
    colors.button_success_hover = Some(hex(p.success));
    colors.button_success_active = Some(hex(p.success));
    colors.button_warning = Some(hex(p.warning));
    colors.button_warning_foreground = Some(fg_on_tint.clone());
    colors.button_warning_hover = Some(hex(p.warning));
    colors.button_warning_active = Some(hex(p.warning));
    colors.button_info = Some(hex(p.info));
    colors.button_info_foreground = Some(fg_on_tint.clone());
    colors.button_info_hover = Some(hex(p.info));
    colors.button_info_active = Some(hex(p.info));
    colors.danger = Some(hex(p.danger));
    colors.danger_foreground = Some(fg_on_tint.clone());
    colors.danger_hover = Some(hex(p.danger));
    colors.danger_active = Some(hex(p.danger));
    colors.success = Some(hex(p.success));
    colors.success_foreground = Some(fg_on_tint.clone());
    colors.success_hover = Some(hex(p.success));
    colors.success_active = Some(hex(p.success));
    colors.warning = Some(hex(p.warning));
    colors.warning_foreground = Some(fg_on_tint.clone());
    colors.warning_hover = Some(hex(p.warning));
    colors.warning_active = Some(hex(p.warning));
    colors.info = Some(hex(p.info));
    colors.info_foreground = Some(fg_on_tint.clone());
    colors.info_hover = Some(hex(p.info));
    colors.info_active = Some(hex(p.info));
    colors.scrollbar = Some(hex(p.canvas));
    colors.scrollbar_thumb = Some(hex(p.scrollbar_thumb));
    colors.scrollbar_thumb_hover = Some(hex(p.hover));

    ThemeConfig {
        name: name.into(),
        mode,
        font_size: Some(metrics::TEXT_LABEL),
        mono_font_size: Some(metrics::TEXT_META),
        radius: Some(metrics::RADIUS_MD as usize),
        radius_lg: Some(metrics::RADIUS_LG as usize),
        shadow: Some(true),
        colors,
        ..Default::default()
    }
}

/// Install both Licoman themes into the global theme and switch to `initial`.
///
/// Call once during startup, before the first window paints, so there is no
/// bright first-frame flash (01_DESIGN_SYSTEM.md §2 contract 6).
pub fn install(cx: &mut gpui_kit::App, initial: ThemeMode) {
    let dark = std::rc::Rc::new(theme_config(ThemeMode::Dark));
    let light = std::rc::Rc::new(theme_config(ThemeMode::Light));
    Theme::update(cx, |theme| {
        theme.apply_config(&light);
        theme.apply_config(&dark);
    });
    Theme::change(initial, None, cx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{px, rgb};

    #[test]
    fn dark_and_light_palettes_differ_everywhere() {
        let d = format!("{DARK:?}");
        let l = format!("{LIGHT:?}");
        assert_ne!(d, l);
        // Every token must be distinct within a palette too, so a typo can't
        // silently alias two semantic roles.
        let vals = [
            DARK.canvas,
            DARK.sidebar,
            DARK.surface,
            DARK.raised,
            DARK.hover,
            DARK.selected,
            DARK.border,
            DARK.text_primary,
            DARK.text_secondary,
            DARK.text_muted,
            DARK.accent,
            DARK.on_accent,
            DARK.success,
            DARK.warning,
            DARK.danger,
        ];
        for (i, a) in vals.iter().enumerate() {
            for b in vals.iter().skip(i + 1) {
                assert_ne!(a, b, "duplicate dark token");
            }
        }
    }

    #[test]
    fn dark_config_carries_licoman_canvas_and_radius() {
        let mut theme = Theme::default();
        theme.apply_config(&std::rc::Rc::new(theme_config(ThemeMode::Dark)));
        assert_eq!(theme.background, rgb(DARK.canvas).into());
        assert_eq!(theme.foreground, rgb(DARK.text_primary).into());
        assert_eq!(theme.primary, rgb(DARK.accent).into());
        assert_eq!(theme.sidebar, rgb(DARK.sidebar).into());
        assert_eq!(theme.border, rgb(DARK.border).into());
        // radius sm = md/2, md = 8, lg = 12, xl = md*2 = 16 per the spec.
        assert_eq!(theme.radius, px(metrics::RADIUS_MD));
        assert_eq!(theme.radius_lg, px(metrics::RADIUS_LG));
        let radii = theme.radius_tokens();
        assert_eq!(radii.sm, px(metrics::RADIUS_MD / 2.));
        assert_eq!(radii.xl, px(metrics::RADIUS_MD * 2.));
        assert_eq!(theme.font_size, px(metrics::TEXT_LABEL));
    }

    #[test]
    fn light_config_carries_licoman_canvas() {
        let mut theme = Theme::default();
        theme.apply_config(&std::rc::Rc::new(theme_config(ThemeMode::Light)));
        assert_eq!(theme.background, rgb(LIGHT.canvas).into());
        assert_eq!(theme.primary, rgb(LIGHT.accent).into());
        assert_eq!(theme.sidebar, rgb(LIGHT.sidebar).into());
        assert_eq!(theme.title_bar, rgb(LIGHT.sidebar).into());
    }

    #[test]
    fn configs_use_licoman_names_and_modes() {
        let dark = theme_config(ThemeMode::Dark);
        assert_eq!(dark.name, THEME_DARK_NAME);
        assert_eq!(dark.mode, ThemeMode::Dark);
        let light = theme_config(ThemeMode::Light);
        assert_eq!(light.name, THEME_LIGHT_NAME);
        assert_eq!(light.mode, ThemeMode::Light);
    }

    #[test]
    fn window_metrics_match_spec() {
        assert_eq!(metrics::HEADER_HEIGHT, 56.);
        assert_eq!(metrics::SIDEBAR_WIDTH, 244.);
        assert_eq!(metrics::APPS_LIST_WIDTH, 260.);
        assert_eq!(metrics::CONTENT_MAX_WIDTH, 800.);
        assert_eq!(metrics::CONTENT_PADDING, 32.);
        assert_eq!(metrics::GROUP_GAP, 24.);
        assert_eq!(metrics::SETTING_ROW_MIN_HEIGHT, 60.);
    }
}
