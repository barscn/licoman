//! Native Xfconf change notifications (Phase 08, Wave A.3): the core's
//! zbus signal watcher thread (`PropertyChanged` / `PropertyRemoved` on
//! `org.xfce.Xfconf`) → debounce thread → futures channel → main-thread
//! task. Same three-piece shape as `gsettings_watch`; each notification
//! triggers a background read pass, and staged edits survive the publish.

use std::sync::mpsc;
use std::time::Duration;

use futures::StreamExt as _;
use futures::channel::mpsc as futures_mpsc;
use gpui_kit::{App, WeakEntity};

use crate::app_state::AppState;

/// Quiet window before a burst of change signals collapses into one refresh.
const DEBOUNCE: Duration = Duration::from_millis(500);

pub struct XfconfWatcher {
    _watch: licoman_core::read::xfconf::XfconfChangeWatch,
    _debounce_thread: std::thread::JoinHandle<()>,
}

/// Start the Xfconf signal watcher. Returns `None` when the session bus or
/// the signal subscription is unavailable — Refresh remains the manual
/// invalidation path.
pub fn start(app: WeakEntity<AppState>, cx: &mut App) -> Option<XfconfWatcher> {
    let (event_tx, event_rx) = mpsc::channel::<()>();
    let watch = licoman_core::read::xfconf::spawn_change_watcher(move || {
        let _ = event_tx.send(());
    })?;

    let (invalidate_tx, invalidate_rx) = futures_mpsc::unbounded::<()>();
    let _debounce_thread = std::thread::Builder::new()
        .name("licoman-xfconf-watch-debounce".into())
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

    Some(XfconfWatcher {
        _watch: watch,
        _debounce_thread,
    })
}
