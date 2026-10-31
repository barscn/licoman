//! Debounced filesystem watching over the read backends' configuration
//! paths (05_READ_ONLY_BACKENDS.md §3: "External edits by another tool
//! should trigger a refresh … Debounce watchers; provide manual Refresh").
//!
//! Same three-piece shape as `environment_watch` (notify thread → debounce
//! thread → futures channel → main-thread task), watching the paths the
//! backend router reports: the dconf user database and the Alacritty
//! config directory. dconf-service rewrites the db file on every
//! `gsettings set`, so an external `gsettings set` becomes a refresh here;
//! editing alacritty.toml in an editor becomes one too.

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

pub struct SettingsWatcher {
    paths: Vec<PathBuf>,
    _watcher: RecommendedWatcher,
    _debounce_thread: std::thread::JoinHandle<()>,
}

impl SettingsWatcher {
    /// The watched paths (for change detection before re-attaching).
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }
}

/// Start watching the backend config paths. Returns `None` when nothing
/// could be watched — a usable state; Refresh remains the invalidation
/// path.
pub fn start(
    app: WeakEntity<AppState>,
    paths: Vec<PathBuf>,
    cx: &mut App,
) -> Option<SettingsWatcher> {
    let (event_tx, event_rx) = mpsc::channel::<()>();
    let mut watcher = notify::recommended_watcher(move |result: Result<Event, notify::Error>| {
        // Only *changes* invalidate; Access events (readers scanning
        // config) do not (Phase 03 lesson).
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
    for path in &paths {
        // Non-recursive for both shapes: the watched set is the exact
        // backend paths (a file and its parent directory), not trees.
        if watcher.watch(path, RecursiveMode::NonRecursive).is_ok() {
            watched += 1;
        }
    }
    if watched == 0 {
        return None;
    }

    let (invalidate_tx, invalidate_rx) = futures_mpsc::unbounded::<()>();
    let _debounce_thread = std::thread::Builder::new()
        .name("licoman-settings-watch".into())
        .spawn(move || {
            while event_rx.recv().is_ok() {
                while event_rx.recv_timeout(DEBOUNCE).is_ok() {}
                std::thread::sleep(DEBOUNCE);
                while event_rx.recv_timeout(Duration::from_millis(1)).is_ok() {}
                if invalidate_tx.unbounded_send(()).is_err() {
                    break;
                }
            }
        })
        .ok()?;

    cx.spawn(async move |cx| {
        let mut invalidations = invalidate_rx;
        while invalidations.next().await.is_some() {
            let alive = app
                .update(cx, |state, cx| state.on_settings_changed_on_disk(cx))
                .is_ok();
            if !alive {
                break;
            }
        }
    })
    .detach();

    Some(SettingsWatcher {
        paths,
        _watcher: watcher,
        _debounce_thread,
    })
}
