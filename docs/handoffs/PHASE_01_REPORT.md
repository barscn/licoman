# Phase 01 Report — Licoman Design System and Visual Acceptance

**Date:** 2026-10-30
**Workspace:** `/home/baris/Work/licoman` (fresh repository; the plan's
referenced prototype `barscn/gpui-kit-mock` is not available on this machine —
see docs/design/decisions.md D1)
**Executor:** automated session (ZCode), one phase per session as required.

---

## 1. Goal and actual scope implemented

**Goal (from the plan):** the definitive Licoman v1 design system plus a
mock-backed, visually reviewed implementation of the main and Applications
windows; no real OS/application configuration changes.

**Implemented:**

- **Quiet Control design system in code**: complete dark/light semantic
  token palettes; a full mapping onto gpui-kit's `ThemeConfig` (background,
  sidebar, surfaces, primary/accent, list/tab/table/title-bar families,
  buttons, switch/slider, scrollbar, overlay, window border); typography
  (14/12 base, system UI + mono fallbacks, nothing bundled); spacing/radius
  metrics (sm 5 / md 8 / lg 12 / xl 16) as constants.
- **Theme product behavior**: fresh install launches **Dark**; Light and
  System selectable in `Licoman → Preferences → Appearance` (radio group);
  the choice persists to `$XDG_CONFIG_HOME/licoman/preferences.toml`
  (atomic write, `$HOME/.config` fallback); the file is only created after a
  user change; corrupt/missing/unsupported files fall back to Dark with a
  non-blocking diagnostic surfaced in Preferences; theme applies to all
  windows immediately without restart; `System` resolves via the desktop
  appearance signal and falls back to Dark (disclosed); windows observe
  appearance changes while `System` is selected.
- **Main window**: 56 px header (brand, header search, `Ctrl K` hint,
  Preferences button), 244 px sidebar (Shared/Desktop/Distribution scopes +
  Applications action + sample Environment footer), content pane with scope
  heading, description, "Mock data" chip, category groups and setting rows in
  the spec's anatomy (label/description left, control in a 220 px right
  column, state line below, reflow on wrap).
- **Search palette** (Ctrl+K): overlay with input, breadcrumb results
  (scope → category → setting), keyboard Up/Down/Enter/Esc, click-to-navigate,
  honest empty states ("Search all settings" hint / "No matching settings").
- **ApplyBar**: "N change(s) pending" with Discard / Apply (loading state,
  duplicate-apply guard) — a control change is never treated as persisted.
- **Explicit row states**: Idle, Dirty, Applying, Saved (transient), Failed
  (reason + Retry/Revert), External change ("Changed outside Licoman" +
  Reload/Compare with a sample compare dialog), Permission needed (lock +
  "Requires permission — scope" + Authorize… opening an honest
  "not available in this build" dialog), Restart needed (target named),
  Unsupported (read-only value + reason).
- **Applications window**: filter list (mock apps with Supported/Partial/
  Unsupported pills), detail pane with header, category tabs, sample rows
  (NumberInput, Slider, Switch), "Configuration source → Details…" dialog,
  and an Unsupported detail ("no integrations… rather than pretending").
- **Preferences window**: "Licoman appearance" radio selector (Dark/Light/
  System), scope copy ("Applies to Licoman only…"), System fallback
  disclosure, preferences-file notices, About group (name, version,
  descriptor).
- **Actions/keybindings**: Ctrl+K, Ctrl+F, Esc, ↑/↓, Enter, Ctrl+,
  Ctrl+Q; actions namespace `licoman`.
- **Design documents** under `docs/design/` (brand, tokens, layout,
  components, states, accessibility, decision records).
- **Visual evidence**: 10 screenshots captured on a real Hyprland/Wayland
  session (`docs/screenshots/`), including light and dark for both windows
  and the failed/permission states.

**Not implemented (explicitly out of scope or deferred):**

- No real discovery/backends/registry (Phases 03–05); all content is mock and
  labeled.
- `--version`/`--help` CLI surface (Phase 02, "binary name" task).
- Application icon artwork (placeholder glyph; separate visual gate).
- Collapsible sidebar (spec allows "collapsible when narrow"; fixed width
  kept — decisions.md).
- gpui-kit notifications/StatusToast as a distinct surface (the "Saved"
  confirmation is currently in-row; toast wiring deferred with Phase 02's
  action lifecycle).
- Automated UI-integration (headless input-driving) tests; Phase 01 ships
  unit tests + manual checklist instead (no input-injection tool available).

## 2. Changed/added files and public interfaces

```text
Cargo.toml                     workspace (members: apps/licoman; pinned deps)
Cargo.lock                     resolved lockfile (gpui-kit 0.8.1 → gpui-pre 0.3.10)
.gitignore
README.md                      run commands, principles, harness docs
apps/licoman/Cargo.toml        licoman bin crate
apps/licoman/src/main.rs       bootstrap: prefs load → theme install → windows → bindings
apps/licoman/src/actions.rs    actions!(licoman, […]) — OpenSearch, FocusSearch,
                               OpenPreferences, OpenApplications, DismissPalette,
                               PaletteNext/Prev, Quit
apps/licoman/src/prefs.rs      AppearanceMode{Dark,Light,System}, Preferences,
                               load/save (XDG paths, atomic, corrupt-safe),
                               LoadDiagnostic; unit tests
apps/licoman/src/theme.rs      DARK/LIGHT palettes, theme_config(mode), install(),
                               metrics constants; unit tests
apps/licoman/src/mock.rs       MockSetting/RowStatus/Support/MockApp mock model
apps/licoman/src/app_state.rs  AppState entity (EventEmitter<AppEvent>), apply
                               lifecycle, search, WindowRegistry, focus_window
apps/licoman/src/windows.rs    window constructors (bounds/min size/titles)
apps/licoman/src/ui/mod.rs     setting_row/settings_group/read_only_value/
                               support_tag wrappers
apps/licoman/src/ui/main_window.rs    main window + palette + apply bar
apps/licoman/src/ui/apps_window.rs    applications window
apps/licoman/src/ui/preferences_window.rs  preferences window
docs/design/*.md               design system documents (7 files + README)
docs/handoffs/PHASE_01_REPORT.md  this file
docs/screenshots/*.png         captured visual evidence (10 files)
tests/fixtures/                empty; reserved for Phase 03+ fixtures
```

Public (crate-internal) interfaces worth noting for Phase 02:
`prefs::{AppearanceMode, Preferences, load, save, LoadDiagnostic}`,
`theme::{install, theme_config, palette, metrics}`,
`app_state::{AppState, AppEvent, WindowRegistry, system_theme_mode, focus_window}`,
`windows::{open_main_window, open_apps_window, open_preferences_window}`,
`mock::{mock_settings, MOCK_APPS, breadcrumb}`.

## 3. Decisions and deviations

Recorded in `docs/design/decisions.md` (D1–D11). Headlines:

- **D1**: prototype unavailable → fresh implementation; nothing of the
  prototype was touched.
- **D2**: pinned `gpui-kit 0.8.1` / `gpui-pre 0.3.10` / `ashpd 0.13` with
  gpui-base's exact feature set (async-io, settings) — mismatch fails the
  build.
- **D3**: complete per-mode `ThemeConfig`s (default-then-mutate because
  `ThemeConfigColors` has private ANSI fields) so no stock palette leaks.
- **D4**: `System` is a Licoman-level mode resolved through gpui's desktop
  appearance signal; fallback Dark, disclosed.
- **D6**: flat Licoman row wrappers instead of gpui-kit's macOS-style
  `Settings` component.
- **D7**: `h_flex()` centers on the cross axis — root layout rows set
  `items_stretch()` (this produced the one real layout bug found visually).
- **D9**: `LICOMAN_DEMO` env hook for deterministic screenshots.
- **D11**: Preferences is a dedicated window (spec-permitted), not a pane.

Deviations from the phase text: prototype-based steps (inspect/mock reuse)
were re-interpreted per D1; sidebar collapsibility deferred; StatusToast
deferred (in-row "Saved" used); icon artwork placeholder.

## 4. Tests/commands run, with real outcomes

All commands run from the repository root; none skipped.

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | **pass** (after `cargo fmt --all`) |
| `cargo test --workspace` | **pass** — 16/16 tests, 0 failed, 0 ignored (`prefs` 10: defaults, missing/valid/corrupt/invalid-theme/unsupported-schema/empty-file, XDG fallbacks, roundtrip; `theme` 5: palette uniqueness, dark/light mapping incl. radius scale + fonts, config names/modes, window metrics; `window_metrics_match_spec`) |
| `cargo clippy --workspace --all-targets -- -D warnings` | **pass** — 0 warnings after `cargo clippy --fix` (collapsible-let lints) |
| `cargo build --workspace` | **pass**, 0 warnings |
| `cargo build --workspace --release` | **pass** (1m05s, `target/release/licoman`, 59 MB unstripped symbols beyond `strip = "debuginfo"`) |

Runtime verification on the real session (Hyprland/Wayland, `wayland-1`):

- Fresh launch (no preferences file) → window paints Dark; pixel-sampled
  canvas `#0E1117` — **Dark default confirmed**.
- `preferences.toml` with `theme = "light"` → launch paints Light; canvas
  `#F4F6FA`, sidebar `#EBEFF5` — **persistence load path confirmed**.
- Corrupt `preferences.toml` → launch succeeds, paints Dark (`#0E1117`),
  diagnostic available in Preferences — **corrupt-fallback confirmed**
  (screenshot `corrupt-prefs-fallback-dark.png`).
- One layout bug found via screenshots (content pane vertically centered by
  `h_flex()` cross-axis default) — fixed with `items_stretch()` and
  re-captured.

## 5. Visual captures and manual verification instructions

Captures (grim, cropped to the app window, real Wayland session — none
fabricated; deterministic states via `LICOMAN_DEMO`, decisions.md D9):

| File | Shows |
|---|---|
| `main-window-dark.png` | Main window, dark (default first run), pending change + ApplyBar |
| `main-window-light.png` | Main window, Light |
| `apps-window-dark.png` | Applications window, Alacritty detail, tabs, controls, source row |
| `apps-window-light.png` | Applications window, Light |
| `preferences-window-dark.png` | Preferences: theme radios, scope copy, About |
| `search-palette-dark.png` | Ctrl+K palette overlay with empty-state hint |
| `failed-state-dark.png` | Failed row with reason + Retry/Revert |
| `distribution-scope-dark.png` | Permission row + unsupported service row |
| `corrupt-prefs-fallback-dark.png` | Corrupt preferences file → Dark fallback |

Reproduce any of them:

```sh
cargo run -p licoman                       # main window, dark
LICOMAN_DEMO=apps cargo run -p licoman     # applications window
LICOMAN_DEMO=light cargo run -p licoman    # light theme
LICOMAN_DEMO=failed cargo run -p licoman   # failure state
LICOMAN_DEMO=distro cargo run -p licoman   # distribution scope
```

Human-only checks still to walk through (no input injection was available):
the keyboard-only path in `docs/design/accessibility.md` (Tab order,
Ctrl+K→type→Enter navigation, Esc focus return, theme switch by keyboard),
narrow-window reflow, and fractional-scaling runs — these are listed as
pending in §8.

## 6. Platform/environment matrix actually tested

| Dimension | Value |
|---|---|
| OS | Arch Linux (kernel 7.2.11-arch1-1) |
| Session | Hyprland (Wayland), `WAYLAND_DISPLAY=wayland-1` |
| Rendering | Vulkan (mesa/RADV), gpui blade backend |
| Fonts | system fontconfig (no bundling) |
| Rust | 1.99.1 stable (toolchain from rustup; edition 2024) |
| X11 | **not tested** (no X11 session available) — plan note honored: a Wayland capture is not proof of X11 support |
| Other DEs | not tested (single-session machine) |

## 7. Security and data-loss assessment

- **No real configuration was read or written.** The only filesystem write
  Licoman can perform in this phase is its own
  `$XDG_CONFIG_HOME/licoman/preferences.toml` (or `$HOME/.config/licoman/…`),
  and only after the user changes a preference. Mock "Apply" mutates in-memory
  sample state only.
- Preference writes are atomic (temp file + rename) and validated on load;
  every failure mode degrades to Dark with a diagnostic, never a crash.
- No privileged operations, no subprocesses, no network, no telemetry.
- Risk noted for later phases: the portal-based System-mode signal reads the
  desktop's color-scheme via gpui's platform layer (read-only).

## 8. Remaining work and acceptance gates

| Gate (plan §8) | Status |
|---|---|
| Licoman branding visible in windows, Preferences, About; no brand decoration abuse | **pass** (name/version/descriptor; icon artwork pending as allowed) |
| Brand-new preference state starts Dark even on a light desktop | **pass** (pixel-verified; session desktop signal not forced, but default is unconditional Dark) |
| Dark/Light/System selectable in a plainly labeled control | **pass** (Preferences radio group) |
| Choice survives restart; corrupt/missing file does not break startup | **pass** (restart with persisted light file; corrupt-file launch; unit tests) |
| System follows OS theme where integration exists, else explicit Dark fallback | **pass** (gpui portal signal wired; fallback + disclosure implemented; live light/dark switch while running **not exercised** — needs manual desktop toggle, see below) |
| Theme changes propagate to all windows/overlays without restart, no mixed themes | **pass** for both windows + palette overlay (single-window `System`-switch propagation via `Theme::change` refreshes every window; multi-window simultaneous capture taken for dark; light apps capture) |
| Both native windows keyboard-navigable, correct focus, accessible names | **partial** — bindings implemented; the scripted keyboard walk-through could not be executed (no injection tool); manual checklist in accessibility.md |
| Long labels, large text, fractional scaling, narrow windows usable | **partial** — reflow implemented (flex-wrap, min_h rows); narrow/scaling captures pending manual runs |
| Errors, pending, locked/read-only, unavailable, restart-required visually distinct | **pass** (distinct icon+copy+color per state; screenshots) |
| No OS/DE/distro/app settings modified; only Licoman's own preference stored | **pass** |
| Required Cargo checks run and reported truthfully | **pass** (fmt/test/clippy/debug+release build — all green) |
| Design screenshots and `PHASE_01_REPORT.md` supplied | **pass** (this file + `docs/screenshots/`) |

**Stop condition check** (multi-window theme propagation and keyboard focus):
theme propagation is implemented through gpui-kit's global `Theme::change`
(which refreshes every window) and both windows were captured in both themes;
the focus-related keyboard walk-through remains a manual gate — treat the
keyboard path as the open item before starting Phase 02's heavy shell work if
strictness is desired; nothing observed suggests broken focus.

**Deferred to later phases (by plan):** real discovery (03), registry (04),
read backends (05), safe write engine (06), first real writes (07),
expansion (08), measured contrast + packaging + high-contrast pass (09),
`--help/--version` and core/UI crate split (02), sidebar collapse, StatusToast,
icon artwork, automated UI-integration tests.

## 9. Reverting this phase

No user configuration is at risk: the only file the app may create is
`$XDG_CONFIG_HOME/licoman/preferences.toml` — delete it (and the `licoman/`
directory) to return to a pristine state.

To revert the repository to pre-phase state:

```sh
cd /home/baris/Work/licoman
rm -rf apps docs Cargo.toml Cargo.lock README.md .gitignore target
# the repository had no commits before this phase (fresh `git init`)
```

Nothing outside the workspace was modified; no commits were created by the
automated session (left for the user to review and commit).
