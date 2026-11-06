//! The main window constructor: geometry, title and decorations per the
//! design spec (01_DESIGN_SYSTEM.md §4). Licoman is a single-window app —
//! Applications and Settings render as panels inside the main window's
//! content area, so this is the only native window the UI ever opens.

use gpui_kit::component::TitleBar;
use gpui_kit::{
    AnyWindowHandle, App, AppContext, Bounds, Entity, Pixels, Point, Size, TitlebarOptions,
    WindowBounds, WindowOptions, px,
};

use crate::app_state::AppState;
use crate::theme::metrics;
use crate::ui::main_window::MainWindow;

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
        // The AppStream/GitHub-reserved namespace (io.github.<user>) —
        // matches the desktop file id so launchers associate windows,
        // icons and actions correctly (docs/architecture/decisions.md D59).
        app_id: Some("io.github.barscn.Licoman".into()),
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
