//! Native GSettings change notifications (07_FIRST_REAL_INTEGRATIONS.md
//! Integration B: "Support external changes via native settings
//! notifications and refresh UI without losing a pending change silently").
//!
//! Same three-piece shape as `settings_watch`: the core's gio signal watcher
//! thread → debounce thread → futures channel → main-thread task. Each
//! notification triggers a background read pass; staged edits survive the
//! publish (`Store::publish_read` keeps pending edits and refreshes the
//! revision, so a silently-moved source surfaces as a conflict at apply
//! time, never as a lost edit).

use std::sync::mpsc;
use std::time::Duration;

use futures::StreamExt as _;
use futures::channel::mpsc as futures_mpsc;
use gpui_kit::{App, WeakEntity};

use crate::app_state::AppState;

/// Quiet window before a burst of change signals collapses into one refresh.
const DEBOUNCE: Duration = Duration::from_millis(500);

pub struct GSettingsWatcher {
    keys: Vec<(String, String)>,
    _watch: licoman_core::read::gsettings::NativeChangeWatch,
    _debounce_thread: std::thread::JoinHandle<()>,
}

impl GSettingsWatcher {
    /// The subscribed (schema, key) pairs (for change detection before
    /// re-attaching).
    pub fn keys(&self) -> &[(String, String)] {
        &self.keys
    }
}

/// Start the native watcher over `keys`. Returns `None` when there is
/// nothing to watch — Refresh remains the manual invalidation path.
pub fn start(
    app: WeakEntity<AppState>,
    keys: Vec<(String, String)>,
    cx: &mut App,
) -> Option<GSettingsWatcher> {
    let (event_tx, event_rx) = mpsc::channel::<()>();
    let watch =
        licoman_core::read::gsettings::spawn_native_change_watcher(keys.clone(), move || {
            let _ = event_tx.send(());
        })?;

    let (invalidate_tx, invalidate_rx) = futures_mpsc::unbounded::<()>();
    let _debounce_thread = std::thread::Builder::new()
        .name("licoman-gsettings-watch-debounce".into())
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

    Some(GSettingsWatcher {
        keys,
        _watch: watch,
        _debounce_thread,
    })
}
