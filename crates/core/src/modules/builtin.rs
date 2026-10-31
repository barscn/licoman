//! Embedded built-in manifests (04_MODULE_SCHEMA_AND_REGISTRY.md §4/§5):
//! compiled into the binary with `include_str!` so they are trusted at
//! **build/release time**, need no runtime module search roots, and can
//! never be overridden by a file that merely shares a name. The manifest
//! files live in the repository under `modules/builtin/`.
//!
//! Contributor flow: add (or edit) a manifest file, list it here, rebuild.
//! No GPUI source changes — that is the phase's acceptance gate. The files
//! themselves are validated by tests on every build (`registry::tests`
//! plus the `builtin_manifests_are_valid` test in `modules/mod.rs`).

/// `(repository path, embedded text)` in deterministic load order.
pub const MANIFESTS: &[(&str, &str)] = &[
    (
        "modules/builtin/shared-appearance.toml",
        include_str!("../../../../modules/builtin/shared-appearance.toml"),
    ),
    (
        "modules/builtin/shared-audio.toml",
        include_str!("../../../../modules/builtin/shared-audio.toml"),
    ),
    (
        "modules/builtin/shared-network.toml",
        include_str!("../../../../modules/builtin/shared-network.toml"),
    ),
    (
        "modules/builtin/shared-power.toml",
        include_str!("../../../../modules/builtin/shared-power.toml"),
    ),
    (
        "modules/builtin/shared-input.toml",
        include_str!("../../../../modules/builtin/shared-input.toml"),
    ),
    (
        "modules/builtin/shared-portal.toml",
        include_str!("../../../../modules/builtin/shared-portal.toml"),
    ),
    (
        "modules/builtin/desktop-session.toml",
        include_str!("../../../../modules/builtin/desktop-session.toml"),
    ),
    (
        "modules/builtin/desktop-hyprland.toml",
        include_str!("../../../../modules/builtin/desktop-hyprland.toml"),
    ),
    (
        "modules/builtin/desktop-gnome-interface.toml",
        include_str!("../../../../modules/builtin/desktop-gnome-interface.toml"),
    ),
    (
        "modules/builtin/distribution-pacman.toml",
        include_str!("../../../../modules/builtin/distribution-pacman.toml"),
    ),
    (
        "modules/builtin/distribution-services.toml",
        include_str!("../../../../modules/builtin/distribution-services.toml"),
    ),
    (
        "modules/builtin/app-alacritty.toml",
        include_str!("../../../../modules/builtin/app-alacritty.toml"),
    ),
    (
        "modules/builtin/app-kitty.toml",
        include_str!("../../../../modules/builtin/app-kitty.toml"),
    ),
    (
        "modules/builtin/app-mpv.toml",
        include_str!("../../../../modules/builtin/app-mpv.toml"),
    ),
    (
        "modules/builtin/app-neovim.toml",
        include_str!("../../../../modules/builtin/app-neovim.toml"),
    ),
];
