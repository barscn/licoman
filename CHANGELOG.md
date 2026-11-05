# Changelog

All notable changes to Licoman are documented here.

## 0.1.0-alpha.0 — 2026-11-04

The first release candidate: a native Linux settings interface built on a
registry-driven architecture, with honest support states throughout.

### Core

- Declarative module schema and registry: data-only, validated manifests
  (closed backend allowlist, shell-payload rejection, located validation
  issues); 17 built-in modules; JSON Schema published (`licoman module
  schema`).
- Read backends: GSettings/dconf (GVDB parsing, schema defaults, dconf
  precedence), configuration files (TOML with Alacritty's documented
  import merge; mpv's documented grammar with top-level-only semantics),
  D-Bus services (power-profiles-daemon; current and legacy names, typed
  properties), Xfconf channels (explicitly read-only, passive probing,
  per-row channel/property validation), compositor diagnostics over the
  documented Hyprland IPC (runtime-only, read-only).
- Safe write engine: core-derived policies (modules cannot weaken them),
  value validation before I/O, fingerprint + semantic conflict checks,
  imported-winner refusal, xattr/ACL preserve-or-refuse, bounded private
  backups, atomic staging with fsync, readback verification with automatic
  restore, redacted operation log (values never log — asserted by tests).
  Live write families: configuration files (version-gated), GSettings
  (GNOME-desktop-gated native writes), PowerProfiles (typed property
  writes, persistence honestly labelled Unknown).
- Environment discovery: os-release, session/desktop identity, desktop
  entries, D-Bus name probes (passive on both buses), GSettings schema
  facts — all redacted and exportable (`licoman --diagnostics`).

### UI

- Two native windows (main + Applications), registry-driven rows, global
  search palette (pre-computed index; measured p95 ≈ 0.4 ms at 1,240
  settings), staged edits with an apply bar, confirmation dialogs naming
  effect and target, Undo and Reset-to-default disclosures, per-row
  Source details, honest unavailable/permission states.
- Virtualized settings content (per-row view entities; scroll position
  survives unrelated re-renders) — verified at 1,240 synthetic settings.
- Quiet Control design system: Dark first-run default; Light and System
  selectable and applied to every window immediately.

### Platform

- Graceful startup diagnostics: no graphical session and missing Vulkan
  prerequisites (loader, driver manifests) produce actionable messages
  with a troubleshooting pointer; graphics-shaped panics append the same
  pointer (`docs/troubleshooting.md`).
- Packaging: Arch PKGBUILD (tested via makepkg), `.desktop` launcher,
  hicolor icons, AppStream metadata, license notices. App ID
  `io.github.barscn.Licoman` (the AppStream-reserved GitHub namespace).

### Known limitations (honest)

- Xfconf is read-only; writes need a provenance/rollback review first.
- KDE Plasma, COSMIC and Sway integrations are not implemented.
- Interactive keyboard-only and screen-reader walkthroughs remain human
  verification items (no input injector in the test environment).
- Power-profiles persistence is labelled Unknown by design.
- .deb/.rpm packages do not exist yet (no untested placeholders shipped).

### Security

- Phase 06 threat model reviewed against the shipped code and extended
  for Phase 08 (`docs/security/module-trust.md`); no manifest can reach
  any command execution, absolute path write or privileged boundary;
  dynamic (user-supplied) module loading is deliberately not implemented.
- The Phase 07 live-verification isolation incident is documented
  (one test key written to the real dconf database during the first
  attempt; recovered immediately via the platform's own reset; fix and
  procedure recorded in the threat model).
