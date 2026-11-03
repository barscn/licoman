# Accessibility and design review — Phase 09 (release gate)

**Scope:** the release-candidate build (0.1.0-alpha.0) on the reference
machine (Arch, Hyprland 0.57, Wayland, Vulkan). This document states what
was verified, how, and what remains a human gate — no invented claims.

## One design system

- Both native windows (main + Applications) and every overlay render from
  the same Quiet Control tokens (`docs/design/`, `crates/ui/src/theme.rs`).
  Verified: Dark, Light and System render every control type —
  `p09-release-main-dark.png`, `p09-release-apps-dark.png`,
  `p09-release-main-light.png`, `p09-release-apps-light.png` are release-
  build captures of both windows in both themes.
- A corrupt preferences file falls back to Dark with a surfaced diagnostic
  (unit-tested; `docs/usage.md` documents the recovery).
- Theme switching at runtime applies to all windows without restart or
  state loss (Phase 01/06/08 demos; the multi-window theme-while-dialog
  check is `p06-writes-light.png`).

## Status never conveyed by color alone

Rows state support and outcome in words ("Supported (read-only)",
"Unavailable", "Applying changes…", the confirmation text), with icons on
warning/permission rows — verified across the capture set; the honest-
state rule (fail closed) is what makes this checkable.

## Keyboard

- The global palette (Ctrl+K) is fully keyboard-driven: type, arrow, Enter
  navigates into scopes or the Applications window (implemented in Phase
  03; exercised by the demo hooks).
- Tab/keyboard traversal across rows, controls and dialogs is provided by
  the gpui focus system and the component set's focus handling; the
  **full keyboard-only workflow** (launch → Preferences → theme → search →
  choose app → inspect → edit → apply/discard → resolve conflict) remains
  a **human verification gate** — the test environment has no input
  injector (a standing, documented limitation since Phase 01). The
  store-level transitions behind every step of that workflow are asserted
  by core tests.

## Screen readers

- gpui ships an AccessKit integration (atspi on Linux) and the component
  layer exposes roles/labels for the controls Licoman uses; the build does
  not disable it.
- What is **not** verified in this release: a scripted AT walkthrough
  (screen reader announcing row roles, values, disabled states, dialog
  focus). Testing that requires a session with Orca and manual driving —
  recorded as the known limitation; no "WCAG-conformant" claim is made.

## Contrast and legibility

- The Quiet Control token set (documented in `docs/design/`) targets
  ≥ 4.5:1 body contrast in both themes; text sizes come from the shared
  metrics, not per-row ad-hoc values.
- The main window renders at narrow widths without clipped destructive
  buttons (rows wrap; the apply bar never truncates its confirmation
  wording — verified in narrow-window captures from Phase 01, unchanged
  layout system).
- Fractional scaling is handled natively by gpui (verified at 1.5× in
  Phase 01's headless-output check; the mechanism is unchanged).

## Motion and fonts

- The UI has no autonomous animation beyond component feedback; "Enable
  animations" rows exist precisely because the app reads the desktop's
  motion preference as a setting — there is no large-motion animation to
  reduce today, and nothing pulses/blinks by itself.
- System font scale: text scaling is read from the desktop where the
  portal exposes it (the `shared.portal.text_scaling` row is a declared
  mock — honest); fontconfig fonts render through cosmic-text at the
  theme metrics.

## Remaining human gates (honest)

1. Full keyboard-only walkthrough on a real session.
2. Orca/AT walkthrough (roles, announcements, dialog focus).
3. Scale-factor sweep beyond the verified 1.0/1.5 (125%, 200%) — the
   mechanism is gpui-native and was verified at 1.5; the sweep itself
   needs hands on a real session.
