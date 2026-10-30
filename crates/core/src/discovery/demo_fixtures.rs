//! Deterministic environment snapshots for visual verification
//! (LICOMAN_DEMO hooks).
//!
//! These build `EnvironmentSnapshot`s by running the **real discovery
//! parsers** over embedded fixture data — never by hand-constructing display
//! strings — so demo screens verify the actual rendering of actual parser
//! output for environments this machine cannot run (Fedora+GNOME, Ubuntu+X11,
//! Debian+Xfce, ambiguous multi-value XDG_CURRENT_DESKTOP).
//!
//! They are visual-verification fixtures, not detection shortcuts: the
//! normal runtime path never calls into this module.

use std::collections::BTreeMap;

use super::EnvironmentSnapshot;
use super::apps::{InstallOrigin, InstalledApp, parse_desktop_entry};
use super::backends::{BackendCapability, BackendKind, BackendStatus};
use super::os_release::{ExecutionContext, KernelInfo, distribution_from_keys, parse_os_release};
use super::session::detect as detect_session;

fn env(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn distro_for(os_release_text: &str) -> super::os_release::DistributionEvidence {
    let (keys, skipped) = parse_os_release(os_release_text);
    distribution_from_keys(&keys, skipped).0
}

fn app_from(entry_text: &str, id: &str, origin: InstallOrigin) -> InstalledApp {
    let entry = parse_desktop_entry(entry_text)
        .unwrap_or_else(|_| panic!("demo fixture entry {id} must parse"));
    InstalledApp {
        id: id.to_string(),
        name: entry.name,
        generic_name: entry.generic_name,
        comment: entry.comment,
        icon: entry.icon,
        origin,
        source_path: std::path::PathBuf::from(format!("/usr/share/applications/{id}")),
        no_display: entry.no_display,
        try_exec_found: true,
        // Demo fixtures are permissive on purpose (no desktop restrictions).
        restricted_from_current: false,
        wm_class: entry.wm_class,
        flatpak_id: entry.flatpak_id,
        evidence: Vec::new(),
    }
}

fn backend(
    id: &str,
    kind: BackendKind,
    available: bool,
    detail: Option<String>,
) -> BackendCapability {
    BackendCapability {
        id: id.to_string(),
        kind,
        status: if available {
            BackendStatus::Available
        } else {
            BackendStatus::Unavailable {
                reason: "not present in this demo environment".to_string(),
            }
        },
        evidence: Vec::new(),
        detail,
    }
}

/// The host-like reference: Arch + Hyprland + Wayland.
pub fn arch_hyprland_wayland() -> EnvironmentSnapshot {
    let (session, _) = detect_session(&env(&[
        ("XDG_CURRENT_DESKTOP", "Hyprland"),
        ("XDG_SESSION_DESKTOP", "Hyprland"),
        ("XDG_SESSION_TYPE", "wayland"),
        ("WAYLAND_DISPLAY", "wayland-1"),
    ]));
    let apps = vec![
        app_from(
            "[Desktop Entry]\nType=Application\nName=Alacritty\nExec=alacritty\nStartupWMClass=Alacritty\n",
            "alacritty.desktop",
            InstallOrigin::User,
        ),
        app_from(
            "[Desktop Entry]\nType=Application\nName=Kitty\nExec=kitty\n",
            "kitty.desktop",
            InstallOrigin::System,
        ),
        app_from(
            "[Desktop Entry]\nType=Application\nName=Zed\nExec=zed\nX-Flatpak=dev.zed.Zed\n",
            "dev.zed.Zed.desktop",
            InstallOrigin::FlatpakSystem,
        ),
        app_from(
            "[Desktop Entry]\nType=Application\nName=VolumeControl\nExec=volctl\nNoDisplay=true\n",
            "volctl.desktop",
            InstallOrigin::System,
        ),
    ];
    EnvironmentSnapshot {
        distro: distro_for(
            "NAME=\"Arch Linux\"\nPRETTY_NAME=\"Arch Linux\"\nID=arch\nID_LIKE=arch\nBUILD_ID=rolling\n",
        ),
        kernel: Some(KernelInfo {
            release: "7.2.11-arch1-1".to_string(),
            arch: "x86_64",
        }),
        execution_context: ExecutionContext::Host,
        session,
        installed_apps: apps,
        available_backends: vec![
            backend(
                "package-manager:pacman",
                BackendKind::PackageTool,
                true,
                None,
            ),
            backend(
                "dbus-session",
                BackendKind::Service,
                true,
                Some("42 names listed".into()),
            ),
            backend(
                "gsettings-schemas",
                BackendKind::SchemaDatabase,
                false,
                None,
            ),
            backend("config:alacritty", BackendKind::AppConfig, true, None),
            backend("config:kitty", BackendKind::AppConfig, true, None),
        ],
        problems: Vec::new(),
    }
}

/// Fedora + GNOME + Wayland.
pub fn fedora_gnome_wayland() -> EnvironmentSnapshot {
    let (session, _) = detect_session(&env(&[
        ("XDG_CURRENT_DESKTOP", "GNOME"),
        ("XDG_SESSION_DESKTOP", "gnome"),
        ("XDG_SESSION_TYPE", "wayland"),
        ("WAYLAND_DISPLAY", "wayland-0"),
    ]));
    EnvironmentSnapshot {
        distro: distro_for(
            "NAME=\"Fedora Linux\"\nVERSION=\"43 (Workstation Edition)\"\nID=fedora\n\
             ID_LIKE=\"fedora rhel\"\nVERSION_ID=43\nPRETTY_NAME=\"Fedora Linux 43 (Workstation Edition)\"\n",
        ),
        kernel: Some(KernelInfo {
            release: "6.17.4-300.fc43.x86_64".to_string(),
            arch: "x86_64",
        }),
        execution_context: ExecutionContext::Host,
        session,
        installed_apps: Vec::new(),
        available_backends: vec![
            backend("package-manager:dnf", BackendKind::PackageTool, true, None),
            backend("dbus-session", BackendKind::Service, true, None),
            backend(
                "gsettings-schemas",
                BackendKind::SchemaDatabase,
                true,
                Some("96 distinct schemas".into()),
            ),
        ],
        problems: Vec::new(),
    }
}

/// Ubuntu + GNOME + X11.
pub fn ubuntu_gnome_x11() -> EnvironmentSnapshot {
    let (session, _) = detect_session(&env(&[
        ("XDG_CURRENT_DESKTOP", "ubuntu:GNOME"),
        ("XDG_SESSION_DESKTOP", "ubuntu"),
        ("XDG_SESSION_TYPE", "x11"),
        ("DISPLAY", ":0"),
    ]));
    EnvironmentSnapshot {
        distro: distro_for(
            "NAME=\"Ubuntu\"\nID=ubuntu\nID_LIKE=debian\nPRETTY_NAME=\"Ubuntu 26.04 LTS\"\nVERSION_ID=\"26.04\"\n",
        ),
        kernel: Some(KernelInfo {
            release: "6.14.0-13-generic".to_string(),
            arch: "x86_64",
        }),
        execution_context: ExecutionContext::Host,
        session,
        installed_apps: Vec::new(),
        available_backends: vec![
            backend("package-manager:apt", BackendKind::PackageTool, true, None),
            backend("dbus-session", BackendKind::Service, true, None),
        ],
        problems: Vec::new(),
    }
}

/// Debian + Xfce (X11) — desktop, distro family and package-manager facts
/// that must not be conflated.
pub fn debian_xfce() -> EnvironmentSnapshot {
    let (session, _) = detect_session(&env(&[
        ("XDG_CURRENT_DESKTOP", "XFCE"),
        ("XDG_SESSION_DESKTOP", "xfce"),
        ("XDG_SESSION_TYPE", "x11"),
        ("DISPLAY", ":0"),
    ]));
    EnvironmentSnapshot {
        distro: distro_for(
            "PRETTY_NAME=\"Debian GNU/Linux trixie (testing)\"\nNAME=\"Debian GNU/Linux\"\nID=debian\nID_LIKE=debian\n",
        ),
        kernel: Some(KernelInfo {
            release: "6.12.0-amd64".to_string(),
            arch: "x86_64",
        }),
        execution_context: ExecutionContext::Host,
        session,
        installed_apps: Vec::new(),
        available_backends: vec![
            backend("package-manager:apt", BackendKind::PackageTool, true, None),
            backend("dbus-session", BackendKind::Service, true, None),
        ],
        problems: Vec::new(),
    }
}

/// Ambiguous session: multi-value `XDG_CURRENT_DESKTOP` disagreeing with
/// `XDG_SESSION_DESKTOP` — the UI must show the ambiguity honestly.
pub fn multi_xdg_ambiguous() -> EnvironmentSnapshot {
    let (session, _) = detect_session(&env(&[
        ("XDG_CURRENT_DESKTOP", "KDE"),
        ("XDG_SESSION_DESKTOP", "gnome"),
        ("XDG_SESSION_TYPE", "wayland"),
        ("WAYLAND_DISPLAY", "wayland-1"),
        ("DISPLAY", ":0"),
    ]));
    EnvironmentSnapshot {
        distro: distro_for("ID=arch\nNAME=\"Arch Linux\"\n"),
        kernel: None,
        execution_context: ExecutionContext::Host,
        session,
        installed_apps: Vec::new(),
        available_backends: Vec::new(),
        problems: Vec::new(),
    }
}

/// Nothing detected at all — the "unknown environment" reference state.
pub fn empty_environment() -> EnvironmentSnapshot {
    EnvironmentSnapshot {
        distro: super::os_release::DistributionEvidence::missing(),
        kernel: None,
        execution_context: ExecutionContext::Unknown,
        session: detect_session(&BTreeMap::new()).0,
        installed_apps: Vec::new(),
        available_backends: Vec::new(),
        problems: Vec::new(),
    }
}

/// Look up a demo snapshot by `LICOMAN_DEMO` fixture name.
pub fn demo_snapshot(name: &str) -> Option<EnvironmentSnapshot> {
    match name {
        "arch-hyprland" => Some(arch_hyprland_wayland()),
        "fedora-gnome" => Some(fedora_gnome_wayland()),
        "ubuntu-x11" => Some(ubuntu_gnome_x11()),
        "debian-xfce" => Some(debian_xfce()),
        "multi-xdg" => Some(multi_xdg_ambiguous()),
        "unknown" => Some(empty_environment()),
        _ => None,
    }
}

/// All demo snapshot names, for `--help`.
pub const DEMO_SNAPSHOT_NAMES: &[&str] = &[
    "arch-hyprland",
    "fedora-gnome",
    "ubuntu-x11",
    "debian-xfce",
    "multi-xdg",
    "unknown",
];
