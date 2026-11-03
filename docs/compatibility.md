# Compatibility and tested support

This matrix carries **only actually tested claims** (Phase 08 acceptance
rule: "Never mark a row as production-supported solely because the module
parses or appears in a mock screenshot"). Every row cites its evidence:
a live run, a screenshot, or the fixture test suite. Untested combinations
are labelled as such — absence of a claim is a claim of no claim.

Reference machine for the live evidence: Arch Linux (x86_64, kernel
7.x), Hyprland 0.57 (Wayland), Mesa/RADV Vulkan, Rust 1.99.1 stable.

## Desktop environments and window managers

| Target | Detected versions | Tested session | Read | Write | Persistence | Known limits |
|---|---|---|---|---|---|---|
| GNOME (GSettings) | glib 2.x-era schemas; GNOME 45+ era | real dconf on Arch/Hyprland (schemas present; reads live; user-approved write round trip on a disposable dconf root — Phase 07 L1) | yes | yes, native gio API, **GNOME-desktop-gated** | durable (dconf) | writes refused outside a GNOME session (fail-closed gate); schema/key presence probed per row |
| Xfce (Xfconf) | xfconf 4.20.0 | Arch/Hyprland, xfconfd running (started explicitly for the test; it exits when idle) | yes — live verified (`p08-live-xfconf.png`) | **no — explicit read-only** | service-managed (xfconfd persists channels); unknown beyond that | channel/property presence validated per row; Licoman never activates the D-Bus-activatable service (proven: full run leaves it stopped) |
| Hyprland (compositor IPC) | 0.56.2 and 0.57.0 | Arch/Hyprland live | yes — runtime diagnostics only (`p07-live-hyprland.png`) | **no — runtime-only read** | runtime only | documented IPC diagnostics (version, workspace, monitors); no config writing, persistence is out of scope by design |
| Sway | not installed here | — | — | — | — | untested; no claims (read-only introspection is the planned first step) |
| KDE Plasma | not installed here | — | — | — | — | untested; KConfig adapter not implemented (planned, version-aware) |
| COSMIC | not installed here | — | — | — | — | untested; no claims |

## Applications

| Target | Detected versions | Tested system | Read | Write | Persistence | Known limits |
|---|---|---|---|---|---|---|
| mpv | 0.41.0 (`--version`, `--list-options` live evidence) | Arch (config created for the test, disclosed in the report) | yes — live verified (`p08-live-mpv.png`) | version-gated (≥ 0.33, fail-closed on unknown) through the safe write engine on fixture roots; live write not executed | durable (file) | top-level keys only (profile sections are never read or edited); small era-stable option group; absent keys are honest "default applies" states |
| Alacritty | not installed here (fixture-verified 0.13-era) | fixture roots | yes (fixtures) | version-validated (≥ 0.13 TOML era) on fixture roots | durable (file) | imports merged last-wins; unknown versions disable writes, reads stay |
| Kitty | not installed here | — | — | — | — | untested; manifest present, mock-only, honestly labelled |
| Neovim | not installed here | — | — | — | — | untested; executable config (Lua) — will be read-only or managed-overlay only, never search-and-replace |

## Shared / system capabilities

| Target | Detected versions | Tested system | Read | Write | Persistence | Known limits |
|---|---|---|---|---|---|---|
| power-profiles-daemon | service absent on the reference machine | typed fake + zbus against the documented interface | yes (service present systems) | typed `Properties.Set` (D52); live write not possible here | **Unknown** (interface documents no guarantee) | holds surfaced as a count only; both current and legacy bus names probed |
| Portals (settings) | xdg-desktop-portal-gtk + hyprland present | live passive probes | partial (declared rows) | no | — | portal rows where the backend is not implemented show declared mock states, labelled as such |

## Distribution

| Target | Detected | Tested | Read | Write | Known limits |
|---|---|---|---|---|---|
| Arch (pacman) | pacman present | live | package-manager capability (informational) | **no — privileged operations disabled by design** | no package install/removal/repo changes without a separately reviewed design |

## Scale and performance (Phase 08 gates)

| Metric | Target | Measured (this machine, release build where noted) |
|---|---|---|
| Search index build, 1,240 synthetic settings | — | 1.42 ms (debug test; release re-measured in the Phase 09 quality gates) |
| Search query p95, 1,240 settings | < 100 ms | **0.38 ms** (`scale_search_latency_1000_settings_100_apps`, 12 queries × 3 runs) |
| Rendered catalog | 1,000+ settings scrollable | 1,000-row category in the main window through the virtualized list (`p08-scale-list.png`); 100+ apps in the Applications window |

## What is deliberately NOT claimed

- "Every Linux setting" — no. Coverage is incremental per backend, and the
  Applications window keeps unsupported-but-installed apps visible with a
  real integration-status explanation.
- Universal file-format editing — TOML and mpv's documented grammar only;
  other formats are recorded but not read.
- Xfconf/GSettings/PowerProfiles writes on arbitrary sessions — each behind
  its own documented gate; Xfconf has no write path at all in this build.
- Any privileged operation (package management, services, firewall).
