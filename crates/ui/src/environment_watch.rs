//! Debounced filesystem watching over the discovered application
//! directories (03_ENVIRONMENT_DISCOVERY.md: "Prefer filesystem
//! notifications with debouncing; avoid high-frequency polling").
//!
//! Three pieces, deliberately dumb:
//! 1. `notify`'s inotify watcher (its own thread) forwards events into a
//!    channel — no filesystem work happens on the UI thread.
//! 2. A debounce thread coalesces bursts (editors write several times) and
//!    hands off one invalidation per quiet 500 ms window.
//! 3. The invalidations travel over a `futures` unbounded channel; a
//!    main-thread task *awaits* them (never blocks the executor) and
//!    refreshes discovery. Zero wakeups when nothing changes; the task ends
//!    when the app shuts down.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use futures::StreamExt as _;
use futures::channel::mpsc as futures_mpsc;
use gpui_kit::{App, WeakEntity};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};

use crate::app_state::AppState;

/// Quiet window before a burst of events collapses into one refresh.
const DEBOUNCE: Duration = Duration::from_millis(500);

pub struct EnvironmentWatcher {
    _watcher: RecommendedWatcher,
    _debounce_thread: std::thread::JoinHandle<()>,
}

/// Start watching the application directories. Returns `None` when nothing
/// could be watched (no directories exist) — a perfectly usable state; the
/// Refresh action remains the invalidation path.
pub fn start(
    app: WeakEntity<AppState>,
    dirs: Vec<PathBuf>,
    cx: &mut App,
) -> Option<EnvironmentWatcher> {
    let (event_tx, event_rx) = mpsc::channel::<()>();
    let mut watcher = notify::recommended_watcher(move |result: Result<Event, notify::Error>| {
        // Only *changes* invalidate: reads (Access events — other
        // programs scan menus all the time) and mere opens do not.
        // Errors (e.g. a watched dir deleted) do invalidate.
        let relevant = match &result {
            Err(_) => true,
            Ok(event) => matches!(
                event.kind,
                notify::EventKind::Create(_)
                    | notify::EventKind::Modify(_)
                    | notify::EventKind::Remove(_)
            ),
        };
        if relevant {
            let _ = event_tx.send(());
        }
    })
    .ok()?;

    let mut watched = 0usize;
    for dir in &dirs {
        if dir.is_dir() && watcher.watch(dir, RecursiveMode::NonRecursive).is_ok() {
            watched += 1;
        }
    }
    if watched == 0 {
        return None;
    }

    // Watcher thread → debounce thread → futures channel → main thread.
    let (invalidate_tx, invalidate_rx) = futures_mpsc::unbounded::<()>();
    let _debounce_thread = std::thread::Builder::new()
        .name("licoman-env-watch".into())
        .spawn(move || {
            while event_rx.recv().is_ok() {
                // Drain the rest of the burst, then wait out one quiet
                // window before invalidating.
                while event_rx.recv_timeout(DEBOUNCE).is_ok() {}
                std::thread::sleep(DEBOUNCE);
                while event_rx.recv_timeout(Duration::from_millis(1)).is_ok() {}
                if invalidate_tx.unbounded_send(()).is_err() {
                    break;
                }
            }
        })
        .ok()?;

    // Await invalidations on the main-thread executor (async — the executor
    // is never blocked) and refresh discovery.
    cx.spawn(async move |cx| {
        let mut invalidations = invalidate_rx;
        while invalidations.next().await.is_some() {
            let alive = app
                .update(cx, |state, cx| state.on_environment_changed_on_disk(cx))
                .is_ok();
            if !alive {
                break;
            }
        }
    })
    .detach();

    Some(EnvironmentWatcher {
        _watcher: watcher,
        _debounce_thread,
    })
}
