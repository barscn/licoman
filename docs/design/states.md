# States

The state contract (plan §5) — each state is visually distinct and copy-labeled;
color is never the only signal (icon + text accompany every status).

| State | Visual | Where exercised |
|---|---|---|
| Idle | current value, no message | every row by default |
| Hover | subtle tint (`hover` token) | sidebar, list rows, palette results |
| Keyboard focus | 2 px high-contrast ring (toolkit focus ring) | all controls |
| Pending | warning icon + "Unsaved change" under row; aggregate "N change(s) pending" in ApplyBar | `desktop.session.gaps` mock row starts pending |
| Applying | "Applying…" + loading Apply button, duplicate apply prevented | Apply action |
| Saved | success icon + "Saved" (transient, returns to Idle) | after mock apply |
| Failed | danger icon + reason + **Retry** / **Revert** buttons | `shared.network.dns` (simulated failure; `LICOMAN_DEMO=failed`) |
| External change | warning icon + "Changed outside Licoman" + **Reload** / **Compare** | `desktop.session.accent` (Compare opens a sample diff dialog) |
| Permission needed | lock icon + "Requires permission — scope" + **Authorize…** (opens an honest dialog: authorization is not available in this build) | `distro.packages.updates` |
| Restart needed | warning icon + "Applies after restarting {target}" | `desktop.session.keyboard_repeat_delay` |
| Unsupported | muted explanation of what is absent; read-only value, not a fake control | `shared.power.profile`, `shared.appearance.animations`, `distro.services.timesync` |

Lifecycle implemented in `app_state.rs`: edit → Dirty → Apply → Applying →
(Saved → Idle | Failed → Retry/Revert). A control value changing is never
treated as a persisted setting; discard returns dirty rows to idle.

Screenshots: `docs/screenshots/failed-state-dark.png`,
`distribution-scope-dark.png` (permission + unsupported),
`main-window-dark.png` (pending + apply bar).
