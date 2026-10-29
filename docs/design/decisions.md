# Decision records — Phase 01

Short records of decisions taken while implementing
`01_DESIGN_SYSTEM.md`, including deviations. Order is chronological.

## D1 — The referenced prototype is not available; build fresh

The plan says to inspect `barscn/gpui-kit-mock` and reuse its working
patterns. That repository is not checked out anywhere on this machine and was
not accessible to the plan's author either (noted in `00_START_HERE.md`).
**Decision:** create the Licoman workspace from scratch in an empty
repository, treating the plan documents as the prototype contract. The
"preserve the prototype" rules remain satisfied vacuously; nothing was
renamed, overwritten or destroyed. **Consequence:** "reuse working patterns"
became "verify the pinned gpui-kit stack end to end before building".

## D2 — Stack pinning: gpui-kit 0.8, gpui-pre 0.3.10

`gpui-kit 0.8.1` (published 2026-10-26) is the facade the plan's referenced
docs describe; it pins a GPUI snapshot (`gpui-pre 0.3.10`, zed@08cbb88) and
re-exports gpui, gpui-base, gpui-component and the Lucide asset set.
`ashpd` is shared with gpui-base and must use the identical feature set
(`default-features = false, features = ["async-io", "settings"]`) or the
build fails with a feature-conflict `compile_error!`. Pinned in
`Cargo.toml`/`Cargo.lock`; recorded for Phase 02's version decision record.

## D3 — Custom ThemeConfig instead of scattering hex codes

gpui-kit's `Theme` supports programmatic `ThemeConfig`s. Both Licoman modes
are built as complete `ThemeConfig`s (`theme::theme_config`), so every
load-bearing color is explicit — unspecified fields would silently keep the
stock shadcn palette. `ThemeConfigColors` has private ANSI/chart fields, so
the configs are built default-then-mutate rather than with struct-update
syntax. Tests assert the mapping (canvas, accent, radius scale, fonts).

## D4 — `System` is a Licoman-level mode, resolved at runtime

gpui's `ThemeMode` is Light|Dark only. Licoman's `AppearanceMode` adds
`System` (serialized as `dark|light|system`); resolution goes through
gpui's desktop appearance signal (`App::window_appearance`,
`Window::observe_window_appearance`), which is wired on Linux to the
XDG settings portal. Unavailable/malformed signal → Dark, disclosed in
Preferences. This keeps the persisted preference stable across platforms
without inventing a third toolkit theme.

## D5 — Theme install happens before the first window opens

`theme::install` applies both configs and switches to the resolved mode
inside `application().run` **before** `open_window`, so the first frame is
already in the chosen theme (no bright flash). Verified: fresh run paints
`#0E1117`; a persisted `theme = "light"` file paints `#F4F6FA`
(pixel-sampled); a corrupt file falls back to Dark and startup continues.

## D6 — Flat rows, not the toolkit's macOS-style `Settings` component

gpui-kit ships a `Settings` component (page sidebar + GroupBox cards). The
Quiet Control spec wants flat grouped rows in Licoman's own sidebar/IA with
exposed per-row state lines. **Decision:** thin Licoman wrappers (`setting_row`,
`settings_group`) over primitives, using the toolkit only for leaf controls.
This also keeps the row-state contract (Failed/External/Permission…) in
Licoman's hands.

## D7 — `h_flex()` centers children; layout rows must stretch

gpui-base's `h_flex()` is documented as "a row that centers its children on
the cross axis". The first floating-window screenshot showed the content pane
vertically centered (overflowing both edges). Root layout rows now set
`items_stretch()` explicitly. Recorded because it will bite every future
window.

## D8 — Palette owns Up/Down/Enter/Esc instead of relying on hidden focus

Palette results are clickable rows with a `palette_selected` index moved by
bound `up`/`down` actions in the `SearchPalette` key context; `Enter` accepts
via the input's `PressEnter` event; `Esc` dismisses and restores focus to the
header search field. Arrow-cycling does not yet scroll a long result list
(fine at current result counts; revisit when the registry grows the index).

## D9 — `LICOMAN_DEMO` visual-verification hook

The session had no keyboard/mouse injection tool, so deterministic states
(palette open, apps window, preferences, light theme, failed row, distro
scope) are reachable via the `LICOMAN_DEMO` environment variable. It is a
developer affordance documented in the README and report — screenshots are
real renders, never fabricated images, and the flag has no effect on normal
launches.

## D10 — Mock content is labeled everywhere

Every mock surface carries a visible "Mock data" chip/caption ("Sample
data — real discovery arrives in Phase 03.", "Sample applications — …",
"Mock value; not persisted in this phase."). Unknown apps render as
**Unsupported** with an explanation instead of fake controls. This is the
plan's truthfulness requirement applied to sample data.

## D11 — `Ctrl+,` as the canonical Preferences surface

Preferences is a dedicated native window (allowed by the spec: "dedicated
in-app view or native window"), opened or focused by the header gear button
and `Ctrl+,`. It is not a third always-open pane.
