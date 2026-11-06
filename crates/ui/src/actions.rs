//! Licoman action definitions. Bound in `main.rs`; handled on window roots.

use gpui_kit::actions;

actions!(
    licoman,
    [
        /// Open the global search palette (Ctrl+K).
        OpenSearch,
        /// Focus the current window's search field (Ctrl+F).
        FocusSearch,
        /// Show Licoman's Settings panel in the content area (Ctrl+,).
        OpenPreferences,
        /// Show the Applications panel in the content area.
        OpenApplications,
        /// Dismiss the search palette (Esc).
        DismissPalette,
        /// Move down in palette results (Down).
        PaletteNext,
        /// Move up in palette results (Up).
        PalettePrev,
        /// Quit the application (Ctrl+Q).
        Quit,
    ]
);
