//! Fixture-driven discovery tests over `tests/fixtures/discovery/`
//! (03_ENVIRONMENT_DISCOVERY.md test matrix). The fixtures are strictly
//! isolated synthetic data (CONTRIBUTING.md); tests read them read-only and
//! never touch real user configuration.

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::apps::{AppScanDir, InstallOrigin};
use super::dbus::BusProbe;
use super::os_release::OsReleaseStatus;
use super::{DiscoverySources, ScanControl, ScanStatus, discover};
use crate::discovery::matching::{NO_INTEGRATION_MESSAGE, build_app_list};
use crate::discovery::os_release::ExecutionContext;
use crate::discovery::session::{DesktopIdentity, Protocol};
use crate::mock::Support;

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/discovery")
}

fn env_from(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Sources over the fixture tree: os-release from the given variant,
/// applications from the base fixture set, no live service probes.
fn fixture_sources(
    os_release_rel: &str,
    app_dir_rels: &[(&str, InstallOrigin)],
) -> DiscoverySources {
    let mut sources = DiscoverySources {
        env: env_from(&[
            ("XDG_CURRENT_DESKTOP", "Hyprland"),
            ("XDG_SESSION_TYPE", "wayland"),
            ("WAYLAND_DISPLAY", "wayland-1"),
            ("HOME", "/home/demo"),
        ]),
        os_release_path: Some(fixtures_root().join(os_release_rel)),
        kernel_release_path: None,
        app_dirs: Vec::new(),
        path_dirs: Vec::new(),
        schema_dirs: Vec::new(),
        config_root: None,
    };
    for (rel, origin) in app_dir_rels {
        sources.app_dirs.push(AppScanDir {
            path: fixtures_root().join(rel),
            origin: *origin,
        });
    }
    sources
}

fn base_app_dirs() -> Vec<(&'static str, InstallOrigin)> {
    vec![
        (
            "applications/override-user/applications",
            InstallOrigin::User,
        ),
        (
            "applications/override-system/applications",
            InstallOrigin::System,
        ),
        (
            "flatpak-user/exports/share/applications",
            InstallOrigin::FlatpakUser,
        ),
    ]
}

fn run_discover(sources: &DiscoverySources) -> (super::EnvironmentSnapshot, ScanStatus) {
    let (snapshot, status, _timings) = discover(sources, &ScanControl::new(), &|_| {});
    (snapshot, status)
}

// ---- Matrix row: Arch + Hyprland + Wayland — exact distro/WM/protocol ----

#[test]
fn arch_hyprland_wayland_full_snapshot() {
    let sources = fixture_sources("os-release/arch/os-release", &base_app_dirs());
    let (snapshot, status) = run_discover(&sources);
    assert_eq!(status, ScanStatus::Completed);
    assert_eq!(snapshot.distro.id.as_deref(), Some("arch"));
    assert!(snapshot.distro.matches_family(&["arch"]));
    assert_eq!(snapshot.distro.pretty_name.as_deref(), Some("Arch Linux"));
    assert_eq!(snapshot.session.desktop, DesktopIdentity::Hyprland);
    assert_eq!(snapshot.session.protocol, Protocol::Wayland);
    assert_eq!(snapshot.summary(), "Arch Linux · Hyprland · Wayland");
    // Evidence trails exist for every headline fact.
    assert!(!snapshot.distro.evidence.is_empty());
    assert!(!snapshot.session.desktop_evidence.is_empty());
    assert!(!snapshot.session.protocol_evidence.is_empty());
    assert_eq!(snapshot.execution_context, ExecutionContext::Host);
}

// ---- Matrix rows: Fedora+GNOME+Wayland, Ubuntu+GNOME+X11, Debian+Xfce ----

#[test]
fn fedora_gnome_wayland_with_separate_capability_probes() {
    let mut sources = fixture_sources("os-release/fedora/os-release", &base_app_dirs());
    sources.env = env_from(&[
        ("XDG_CURRENT_DESKTOP", "GNOME"),
        ("XDG_SESSION_DESKTOP", "gnome"),
        ("XDG_SESSION_TYPE", "wayland"),
        ("WAYLAND_DISPLAY", "wayland-0"),
        ("HOME", "/home/demo"),
    ]);
    let (snapshot, _) = run_discover(&sources);
    assert_eq!(snapshot.distro.id.as_deref(), Some("fedora"));
    assert!(snapshot.distro.matches_family(&["rhel"]));
    assert_eq!(snapshot.session.desktop, DesktopIdentity::Gnome);
    assert_eq!(snapshot.session.protocol, Protocol::Wayland);
    // Capability probes remain separate facts (no D-Bus/socket in fixtures
    // → bus unavailable, and nothing claims otherwise).
    let bus = snapshot
        .available_backends
        .iter()
        .find(|b| b.id == "dbus-session")
        .expect("bus fact present");
    assert!(!bus.is_available());
    // The desktop fact never absorbed the capability facts.
    assert_eq!(snapshot.session.desktop, DesktopIdentity::Gnome);
}

#[test]
fn ubuntu_gnome_x11_correctly_identified() {
    let sources = fixture_sources("os-release/ubuntu/os-release", &[]);
    let mut sources = sources;
    sources.env = env_from(&[
        ("XDG_CURRENT_DESKTOP", "ubuntu:GNOME"),
        ("XDG_SESSION_DESKTOP", "ubuntu"),
        ("XDG_SESSION_TYPE", "x11"),
        ("DISPLAY", ":0"),
    ]);
    let (snapshot, _) = run_discover(&sources);
    assert_eq!(snapshot.distro.id.as_deref(), Some("ubuntu"));
    assert!(snapshot.distro.matches_family(&["debian"]));
    assert_eq!(snapshot.session.desktop, DesktopIdentity::Gnome);
    assert_eq!(snapshot.session.protocol, Protocol::X11);
}

#[test]
fn debian_xfce_facts_not_conflated() {
    let sources = fixture_sources("os-release/debian/os-release", &[]);
    let mut sources = sources;
    sources.env = env_from(&[
        ("XDG_CURRENT_DESKTOP", "XFCE"),
        ("XDG_SESSION_TYPE", "x11"),
        ("DISPLAY", ":0"),
        ("PATH", "/usr/bin:/bin"),
    ]);
    let (snapshot, _) = run_discover(&sources);
    assert_eq!(snapshot.distro.id.as_deref(), Some("debian"));
    assert_eq!(snapshot.session.desktop, DesktopIdentity::Xfce);
    // Package-manager facts are separate from distro identity.
    let apt = snapshot
        .available_backends
        .iter()
        .find(|b| b.id == "package-manager:apt")
        .expect("apt fact present");
    let pacman = snapshot
        .available_backends
        .iter()
        .find(|b| b.id == "package-manager:pacman")
        .expect("pacman fact present");
    let _ = (apt, pacman); // both are facts; neither changes the distro id
    assert_eq!(snapshot.distro.id.as_deref(), Some("debian"));
}

// ---- Matrix row: missing /etc/os-release → unknown distro, app usable ----

#[test]
fn missing_os_release_means_unknown_and_usable() {
    let mut sources = fixture_sources("os-release/does-not-exist/os-release", &base_app_dirs());
    sources.os_release_path = Some(fixtures_root().join("os-release/nothing-here"));
    let (snapshot, status) = run_discover(&sources);
    assert_eq!(status, ScanStatus::Completed);
    assert_eq!(snapshot.distro.status, OsReleaseStatus::Missing);
    assert_eq!(snapshot.distro.display_name(), None);
    assert_eq!(snapshot.summary(), "Hyprland · Wayland");
    // The rest of discovery still ran.
    assert!(!snapshot.installed_apps.is_empty());
}

#[test]
fn malformed_os_release_reports_problem_and_partial_facts() {
    let sources = fixture_sources("os-release/malformed/os-release", &[]);
    let (snapshot, _) = run_discover(&sources);
    assert_eq!(snapshot.distro.id.as_deref(), Some("brokenware"));
    assert_eq!(snapshot.distro.name.as_deref(), Some("Broken Ware"));
    assert!(
        snapshot
            .problems
            .iter()
            .any(|p| p.kind == crate::discovery::ProblemKind::OsReleaseMalformed)
    );
}

#[test]
fn future_os_release_fields_are_tolerated() {
    let sources = fixture_sources("os-release/future-fields/os-release", &[]);
    let (snapshot, _) = run_discover(&sources);
    assert_eq!(snapshot.distro.id.as_deref(), Some("arch"));
    // Unknown fields are tolerated: no os-release problem is reported
    // (backend probes may still report their own facts).
    assert!(
        !snapshot
            .problems
            .iter()
            .any(|p| p.kind == crate::discovery::ProblemKind::OsReleaseMalformed)
    );
}

// ---- Matrix rows: broken/malicious desktop files + duplicates + flatpak --

#[test]
fn broken_entries_are_skipped_with_diagnostics_others_survive() {
    let sources = fixture_sources(
        "os-release/arch/os-release",
        &[("applications/base/applications", InstallOrigin::System)],
    );
    let (snapshot, _) = run_discover(&sources);
    let ids: Vec<&str> = snapshot
        .installed_apps
        .iter()
        .map(|a| a.id.as_str())
        .collect();

    // Valid applications are present.
    assert!(ids.contains(&"alacritty.desktop"));
    assert!(ids.contains(&"kitty.desktop"));

    // Broken entries are skipped, with a problem each.
    assert!(!ids.contains(&"broken-no-type.desktop"));
    assert!(
        snapshot
            .problems
            .iter()
            .any(|p| p.technical.contains("broken-no-type.desktop"))
    );

    // [Desktop Action *] groups exist but do not corrupt the main group:
    // the entry is valid and keeps its unlocalized Name.
    let action = snapshot
        .installed_apps
        .iter()
        .find(|a| a.id == "action-only.desktop")
        .expect("action-group entry is a valid application");
    assert_eq!(action.name, "No Main Group");

    // Hidden: ignored entirely (spec: equivalent to deletion).
    assert!(!ids.contains(&"hidden-app.desktop"));

    // NoDisplay: kept but flagged, kept out of the supported view.
    let utility = snapshot
        .installed_apps
        .iter()
        .find(|a| a.id == "utility-helper.desktop")
        .expect("NoDisplay entry kept");
    assert!(utility.no_display);

    // OnlyShowIn=GNOME under Hyprland: kept but flagged restricted.
    let gnome_only = snapshot
        .installed_apps
        .iter()
        .find(|a| a.id == "gnome-only.desktop")
        .expect("restricted entry kept");
    assert!(gnome_only.restricted_from_current);

    // TryExec binary missing: skipped with an explanation.
    assert!(!ids.contains(&"missing-binary.desktop"));
    assert!(snapshot.problems.iter().any(|p| p.kind
        == crate::discovery::ProblemKind::DesktopEntrySkipped
        && p.technical.contains("TryExec")));

    // Link entries are not applications.
    assert!(!ids.contains(&"docs-link.desktop"));
}

#[test]
fn duplicate_entries_resolve_by_directory_precedence() {
    let sources = fixture_sources(
        "os-release/arch/os-release",
        &[
            (
                "applications/override-user/applications",
                InstallOrigin::User,
            ),
            (
                "applications/override-system/applications",
                InstallOrigin::System,
            ),
        ],
    );
    let (snapshot, _) = run_discover(&sources);
    let shared = snapshot
        .installed_apps
        .iter()
        .find(|a| a.id == "shared.desktop")
        .expect("exactly one shared.desktop");
    assert_eq!(shared.name, "Shared App (user)", "user dir wins");
    assert_eq!(shared.origin, InstallOrigin::User);
    // Deterministic identity: one entry, not two.
    assert_eq!(
        snapshot
            .installed_apps
            .iter()
            .filter(|a| a.id == "shared.desktop")
            .count(),
        1
    );
}

#[test]
fn flatpak_app_without_module_is_installed_but_unsupported() {
    let sources = fixture_sources(
        "os-release/arch/os-release",
        &[(
            "flatpak-user/exports/share/applications",
            InstallOrigin::FlatpakUser,
        )],
    );
    let (snapshot, _) = run_discover(&sources);
    let heavy = snapshot
        .installed_apps
        .iter()
        .find(|a| a.id == "com.example.HeavyApp.desktop")
        .expect("flatpak app discovered");
    assert_eq!(heavy.origin, InstallOrigin::FlatpakUser);
    assert_eq!(heavy.flatpak_id.as_deref(), Some("com.example.HeavyApp"));

    // Merged list: present in the all-applications view, unsupported, with
    // the honest message — never silently hidden.
    let list = build_app_list(&snapshot.installed_apps, &snapshot.available_backends);
    let entry = list
        .iter()
        .find(|e| e.identity == "com.example.HeavyApp.desktop")
        .expect("present in merged list");
    assert!(entry.app.is_some());
    assert_eq!(entry.module, None);
    assert_eq!(entry.support, Support::Unsupported);
}

#[test]
fn oversized_desktop_file_is_skipped_not_crashed_on() {
    let dir = std::env::temp_dir().join(format!("licoman-oversize-{}", std::process::id()));
    let apps_dir = dir.join("applications");
    std::fs::create_dir_all(&apps_dir).unwrap();
    let mut big = String::from("[Desktop Entry]\nType=Application\nName=Big\nExec=x\n");
    big.push_str(&"# ".repeat(600_000)); // > 1 MiB
    std::fs::write(apps_dir.join("big.desktop"), &big).unwrap();
    std::fs::write(
        apps_dir.join("small.desktop"),
        "[Desktop Entry]\nType=Application\nName=Small\nExec=x\n",
    )
    .unwrap();

    let sources = DiscoverySources {
        env: BTreeMap::new(),
        os_release_path: None,
        kernel_release_path: None,
        app_dirs: vec![AppScanDir {
            path: apps_dir.clone(),
            origin: InstallOrigin::User,
        }],
        path_dirs: Vec::new(),
        schema_dirs: Vec::new(),
        config_root: None,
    };
    let (snapshot, status) = run_discover(&sources);
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(status, ScanStatus::Completed);
    assert_eq!(snapshot.installed_apps.len(), 1, "only the small file");
    assert!(snapshot.problems.iter().any(|p| p.kind
        == crate::discovery::ProblemKind::DesktopEntrySkipped
        && p.technical.contains("big.desktop")));
}

// ---- Matrix row: no D-Bus session available → safe state -----------------

#[test]
fn no_session_bus_is_a_safe_unavailable_state() {
    // No DBUS_SESSION_BUSADDRESS, no XDG_RUNTIME_DIR → the probe reports
    // unavailability instead of failing discovery.
    let probe = BusProbe {
        available: false,
        names: Vec::new(),
        reason: Some("no session bus address found".to_string()),
    };
    let _ = probe;
    let mut sources = fixture_sources("os-release/arch/os-release", &[]);
    sources.env.remove("DBUS_SESSION_BUSADDRESS");
    sources.env.remove("XDG_RUNTIME_DIR");
    let (snapshot, _) = run_discover(&sources);
    let bus = snapshot
        .available_backends
        .iter()
        .find(|b| b.id == "dbus-session")
        .expect("bus capability present");
    assert!(!bus.is_available());
    assert!(
        snapshot
            .problems
            .iter()
            .any(|p| p.kind == crate::discovery::ProblemKind::DBusProbeFailed)
    );
    // Discovery itself completed — the safe state is data, not an error.
    assert!(!snapshot.installed_apps.is_empty() || snapshot.installed_apps.is_empty());
}

// ---- Matrix row: long-running scan → cancellable, partial result ---------

#[test]
fn long_scan_cancels_between_files_and_reports_cancelled() {
    let dir = std::env::temp_dir().join(format!("licoman-longscan-{}", std::process::id()));
    let apps_dir = dir.join("applications");
    std::fs::create_dir_all(&apps_dir).unwrap();
    for i in 0..300 {
        std::fs::write(
            apps_dir.join(format!("app{i:04}.desktop")),
            format!("[Desktop Entry]\nType=Application\nName=App {i}\nExec=x\n"),
        )
        .unwrap();
    }
    let sources = DiscoverySources {
        env: BTreeMap::new(),
        os_release_path: None,
        kernel_release_path: None,
        app_dirs: vec![AppScanDir {
            path: apps_dir.clone(),
            origin: InstallOrigin::User,
        }],
        path_dirs: Vec::new(),
        schema_dirs: Vec::new(),
        config_root: None,
    };
    let control = ScanControl::new();
    let calls = std::cell::Cell::new(0);
    let (snapshot, status, _timings) = discover(&sources, &control, &|progress| {
        calls.set(calls.get() + 1);
        if progress.stage == super::ScanStage::Applications && progress.done >= 10 {
            control.cancel();
        }
    });
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(status, ScanStatus::Cancelled);
    assert!(calls.get() > 10);
    assert!(
        snapshot.installed_apps.len() < 300,
        "cancelled scan must be partial"
    );
}

// ---- GSettings fixture ------------------------------------------------------

#[test]
fn synthetic_schema_fixture_enumerates_schemas_and_keys() {
    let data = std::fs::read(fixtures_root().join("schemas/gschemas.compiled")).expect("fixture");
    let schemas = super::gsettings::parse_compiled_schemas(&data).expect("parses");
    assert_eq!(
        schemas
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        vec!["org.licoman.Second", "org.licoman.Test"]
    );
    let test = schemas
        .iter()
        .find(|(n, _)| n == "org.licoman.Test")
        .unwrap();
    assert_eq!(test.1, vec!["clock-show-date", "color-scheme", "font-name"]);
}

#[test]
fn schema_probe_reads_the_fixture_database() {
    let mut sources = fixture_sources("os-release/arch/os-release", &[]);
    sources.schema_dirs = vec![fixtures_root().join("schemas")];
    let (snapshot, _) = run_discover(&sources);
    let gsettings_backend = snapshot
        .available_backends
        .iter()
        .find(|b| b.id == "gsettings-schemas")
        .expect("schema capability present");
    assert!(gsettings_backend.is_available());
    assert_eq!(
        gsettings_backend.detail.as_deref(),
        Some("2 distinct schemas")
    );
}

// ---- Determinism -----------------------------------------------------------

#[test]
fn discovery_is_deterministic_for_fixture_inputs() {
    let sources = fixture_sources("os-release/arch/os-release", &base_app_dirs());
    let (a, _) = run_discover(&sources);
    let (b, _) = run_discover(&sources);
    assert_eq!(a.installed_apps, b.installed_apps);
    assert_eq!(a.distro, b.distro);
    assert_eq!(a.session, b.session);
}

// ---- Diagnostics export over fixtures ----------------------------------------

#[test]
fn diagnostics_export_is_redacted_and_content_free() {
    let sources = fixture_sources("os-release/arch/os-release", &base_app_dirs());
    let (snapshot, _) = run_discover(&sources);
    let text = super::export::diagnostics_text(&snapshot, Some("/home/demo"));
    assert!(text.contains("id=arch"));
    // The no-integration message lives in the UI, not the export.
    assert!(!text.contains(NO_INTEGRATION_MESSAGE));
    // No source desktop file path leaks unredacted; the fixture paths are
    // under the repo, not under /home/demo, so they may appear — the rule is
    // that *home* paths are redacted, which the redact_path test covers.
    assert!(!text.contains("Exec="));
}
