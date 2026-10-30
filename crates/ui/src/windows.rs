//! Window constructors: geometry, titles and decorations per the design spec
//! (01_DESIGN_SYSTEM.md §4). Both native windows share theme and state but
//! keep their own selection and search.

use gpui_kit::component::TitleBar;
use gpui_kit::{
    AnyWindowHandle, App, AppContext, Bounds, Entity, Pixels, Point, Size, TitlebarOptions,
    WindowBounds, WindowOptions, px,
};

use crate::app_state::AppState;
use crate::theme::metrics;
use crate::ui::apps_window::AppsWindow;
use crate::ui::main_window::MainWindow;
use crate::ui::preferences_window::PreferencesWindow;

pub fn window_options(size: Size<Pixels>, min: Size<Pixels>, title: &'static str) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            Point {
                x: px(0.),
                y: px(0.),
            },
            size,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some(title.into()),
            appears_transparent: true,
            traffic_light_position: None,
        }),
        window_min_size: Some(min),
        app_id: Some("licoman".into()),
        ..TitleBar::window_options()
    }
}

pub fn open_main_window(
    app: &Entity<AppState>,
    cx: &mut App,
) -> (AnyWindowHandle, Entity<MainWindow>) {
    let app = app.clone();
    let (handle, entity) = gpui_kit::open_window(
        window_options(
            metrics::MAIN_WINDOW_SIZE,
            metrics::MAIN_WINDOW_MIN,
            "Licoman",
        ),
        cx,
        |window, cx| cx.new(|cx| MainWindow::new(app, window, cx)),
    )
    .expect("failed to open main window");
    (handle, entity)
}

pub fn open_apps_window(app: &Entity<AppState>, cx: &mut App) -> AnyWindowHandle {
    open_apps_window_returning_entity(app, cx).0
}

/// Like [`open_apps_window`] but also returns the window entity, for demo
/// hooks that need to adjust view state after opening.
pub fn open_apps_window_returning_entity(
    app: &Entity<AppState>,
    cx: &mut App,
) -> (AnyWindowHandle, Entity<AppsWindow>) {
    let app = app.clone();
    let (handle, entity) = gpui_kit::open_window(
        window_options(
            metrics::APPS_WINDOW_SIZE,
            metrics::APPS_WINDOW_MIN,
            "Applications — Licoman",
        ),
        cx,
        |window, cx| cx.new(|cx| AppsWindow::new(app, window, cx)),
    )
    .expect("failed to open applications window");
    (handle, entity)
}

pub fn open_preferences_window(app: &Entity<AppState>, cx: &mut App) -> AnyWindowHandle {
    let app = app.clone();
    let (handle, _) = gpui_kit::open_window(
        window_options(
            metrics::PREFS_WINDOW_SIZE,
            Size {
                width: px(520.),
                height: px(360.),
            },
            "Licoman Preferences",
        ),
        cx,
        |window, cx| cx.new(|cx| PreferencesWindow::new(app, window, cx)),
    )
    .expect("failed to open preferences window");
    handle
}
