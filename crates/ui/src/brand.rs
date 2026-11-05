//! Brand assets — the Licoman "Modular L" mark.
//!
//! The mark is the Lucide `blocks` glyph (ISC-licensed; see
//! `assets/icons/LUCIDE_LICENSE_ISC.txt` and `NOTICES`). The tintable SVG
//! draws with `currentColor`, so it follows the icon's `text_color`.

use gpui_kit::component::Icon;

/// The tintable brand mark, embedded at compile time from the logo set.
const MARK_TINTABLE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/branding/mark/licoman-mark-tintable.svg"
));

/// The brand mark as a tintable icon sized to the surrounding text.
pub fn brand_mark() -> Icon {
    Icon::default().data(MARK_TINTABLE)
}
