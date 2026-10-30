//! Read-only environment, service and installed-application discovery
//! (03_ENVIRONMENT_DISCOVERY.md).
//!
//! Everything in this module is **passive**: it parses `/etc/os-release`,
//! reads environment-variable snapshots, enumerates XDG `.desktop` entries,
//! and probes for backend services (D-Bus session bus, GSettings schema
//! databases) without launching processes, daemons or changing any state.
//! Desktop-entry `Exec` lines are never executed and never stored.
//!
//! Structure:
//!
//! - [`os_release`]: distribution facts from `/etc/os-release` + kernel info.
//! - [`session`]: desktop/session facts from environment-variable hints.
//! - [`apps`]: installed-application enumeration from XDG data directories.
//! - [`backends`]: passive backend/service capability probes.
//! - [`matching`]: merge of discovered apps with registered mock modules.
//! - [`export`]: redacted, non-secret diagnostics text.
//!
//! Threading contract: discovery is a plain synchronous function over
//! [`DiscoverySources`]; callers run it off the UI thread and pass a
//! [`ScanControl`] to cancel it between stages and file batches. Core owns no
//! threads, timers or caches — the caching/refresh contract lives with the
//! caller (UI adapter), which must never pretend cached *setting values* are
//! fresh (those have separate contracts from Phase 05).

pub mod apps;
pub mod backends;
pub mod dbus;
pub mod demo_fixtures;
pub mod export;
pub mod gsettings;
pub mod matching;
pub mod os_release;
pub mod session;

#[cfg(test)]
#[path = "fixtures_tests.rs"]
mod fixtures_tests;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::log;

pub use apps::{InstallOrigin, InstalledApp};
pub use backends::{BackendCapability, BackendKind, BackendStatus};
pub use matching::{AppListEntry, build_app_list};
pub use os_release::{DistributionEvidence, ExecutionContext, KernelInfo, OsReleaseStatus};
pub use session::{DesktopIdentity, Protocol, SessionEvidence};

/// Where one piece of discovery evidence came from. Diagnostics and tests
/// key off this instead of free-form strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvidenceSource {
    OsRelease,
    KernelProc,
    EnvVar(&'static str),
    DesktopEntry,
    PathLookup,
    DBus,
    GSettings,
    ConfigDir,
    FilesystemMarker,
}

impl EvidenceSource {
    /// Short stable name used in logs and the diagnostics export.
    pub fn as_str(&self) -> &'static str {
        match self {
            EvidenceSource::OsRelease => "os-release",
            EvidenceSource::KernelProc => "kernel-proc",
            EvidenceSource::EnvVar(_) => "env-var",
            EvidenceSource::DesktopEntry => "desktop-entry",
            EvidenceSource::PathLookup => "path-lookup",
            EvidenceSource::DBus => "dbus-probe",
            EvidenceSource::GSettings => "gsettings-probe",
            EvidenceSource::ConfigDir => "config-dir",
            EvidenceSource::FilesystemMarker => "fs-marker",
        }
    }
}

/// How reliable one piece of evidence is. Environment variables are *hints*,
/// never authority — most session evidence is therefore Medium or Low.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    High,
    Medium,
    Low,
}

impl Confidence {
    pub fn as_str(&self) -> &'static str {
        match self {
            Confidence::High => "high",
            Confidence::Medium => "medium",
            Confidence::Low => "low",
        }
    }
}

/// One grounded fact with its source, detail and reliability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    pub source: EvidenceSource,
    pub detail: String,
    pub confidence: Confidence,
}

impl Evidence {
    pub fn new(source: EvidenceSource, confidence: Confidence, detail: impl Into<String>) -> Self {
        Self {
            source,
            detail: detail.into(),
            confidence,
        }
    }
}

/// Something went wrong during discovery. Fail-closed: a problem never
/// aborts the scan and never masquerades as a supported value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectionProblem {
    pub kind: ProblemKind,
    /// User-safe, one-line description.
    pub message: String,
    /// Technical detail for the diagnostics export (ids/paths only, never
    /// file contents).
    pub technical: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProblemKind {
    OsReleaseMalformed,
    DesktopEntryInvalid,
    DesktopEntrySkipped,
    DirUnreadable,
    DBusProbeFailed,
    GSettingsProbeFailed,
}

impl ProblemKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProblemKind::OsReleaseMalformed => "os-release-malformed",
            ProblemKind::DesktopEntryInvalid => "desktop-entry-invalid",
            ProblemKind::DesktopEntrySkipped => "desktop-entry-skipped",
            ProblemKind::DirUnreadable => "dir-unreadable",
            ProblemKind::DBusProbeFailed => "dbus-probe-failed",
            ProblemKind::GSettingsProbeFailed => "gsettings-probe-failed",
        }
    }
}

/// Cancellation flag for a running scan. Checked between discovery stages
/// and between file batches inside the application scan; when set, the scan
/// stops promptly and reports [`ScanStatus::Cancelled`].
#[derive(Debug, Default)]
pub struct ScanControl {
    cancelled: AtomicBool,
}

impl ScanControl {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

/// Coarse progress for UIs that want to show movement during long scans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanProgress {
    pub stage: ScanStage,
    /// Entries seen so far in this stage (meaning is stage-specific).
    pub done: usize,
    /// Known total for the stage, if computable (application files).
    pub total: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanStage {
    Distribution,
    Session,
    Applications,
    Backends,
}

/// Inputs for a discovery run. Built from the real environment by
/// [`DiscoverySources::from_environment`] or injected by tests/fixtures so
/// every parser stays pure and deterministic.
#[derive(Debug, Clone, Default)]
pub struct DiscoverySources {
    /// Snapshot of relevant environment variables (`XDG_*`, `DESKTOP_*`,
    /// `WAYLAND_DISPLAY`, `DISPLAY`, `DBUS_SESSION_BUSADDRESS`, `HOME`).
    pub env: BTreeMap<String, String>,
    /// `/etc/os-release` (or an equivalent fixture path).
    pub os_release_path: Option<PathBuf>,
    /// `/proc/sys/kernel/osrelease` (informational kernel identity).
    pub kernel_release_path: Option<PathBuf>,
    /// Application directories **in XDG precedence order** (first wins).
    pub app_dirs: Vec<apps::AppScanDir>,
    /// Directories searched for binaries (TryExec resolution, package
    /// managers) — the `PATH` snapshot.
    pub path_dirs: Vec<PathBuf>,
    /// GSettings schema directories in precedence order.
    pub schema_dirs: Vec<PathBuf>,
    /// Config-location probe roots (usually `$HOME/.config`).
    pub config_root: Option<PathBuf>,
}

impl DiscoverySources {
    /// Capture the real environment. Purely read-only; missing variables are
    /// simply absent from the snapshot.
    pub fn from_environment() -> Self {
        let var = |name: &str| std::env::var(name).ok();
        let env: BTreeMap<String, String> = [
            "XDG_CURRENT_DESKTOP",
            "DESKTOP_SESSION",
            "XDG_SESSION_DESKTOP",
            "XDG_SESSION_TYPE",
            "WAYLAND_DISPLAY",
            "WAYLAND_SOCKET",
            "DISPLAY",
            "DBUS_SESSION_BUSADDRESS",
            "XDG_DATA_HOME",
            "XDG_DATA_DIRS",
            "XDG_CONFIG_HOME",
            "XDG_RUNTIME_DIR",
            "HOME",
            "PATH",
        ]
        .iter()
        .filter_map(|name| var(name).map(|v| (name.to_string(), v)))
        .collect();

        let env_path = |name: &str| std::env::var_os(name).map(PathBuf::from);

        let os_release_path = Some(PathBuf::from("/etc/os-release"));
        let kernel_release_path = Some(PathBuf::from("/proc/sys/kernel/osrelease"));

        // XDG data directories in precedence order (user dir first, then
        // $XDG_DATA_DIRS in order, defaults per the basedir spec).
        let mut data_dirs: Vec<PathBuf> = Vec::new();
        if let Some(dir) = env_path("XDG_DATA_HOME") {
            if dir.is_absolute() {
                data_dirs.push(dir);
            }
        } else if let Some(home) = env_path("HOME") {
            data_dirs.push(home.join(".local/share"));
        }
        if let Some(dirs) = var("XDG_DATA_DIRS") {
            for dir in std::env::split_paths(&dirs) {
                if dir.is_absolute() {
                    data_dirs.push(dir);
                }
            }
        } else {
            data_dirs.push(PathBuf::from("/usr/local/share"));
            data_dirs.push(PathBuf::from("/usr/share"));
        }

        // Flatpak export directories carry their own .desktop files; the
        // user-level export shadows the system one per the flatpak spec.
        let mut app_dirs: Vec<apps::AppScanDir> = Vec::new();
        if let Some(data) = data_dirs.first() {
            app_dirs.push(apps::AppScanDir {
                path: data.join("flatpak/exports/share/applications"),
                origin: InstallOrigin::FlatpakUser,
            });
        }
        app_dirs.push(apps::AppScanDir {
            path: PathBuf::from("/var/lib/flatpak/exports/share/applications"),
            origin: InstallOrigin::FlatpakSystem,
        });
        for (index, dir) in data_dirs.iter().enumerate() {
            app_dirs.push(apps::AppScanDir {
                path: dir.join("applications"),
                // Per the basedir spec the user dir is $XDG_DATA_HOME (or
                // $HOME/.local/share) and comes first; the rest are system.
                origin: if index == 0 {
                    InstallOrigin::User
                } else {
                    InstallOrigin::System
                },
            });
        }

        let path_dirs: Vec<PathBuf> = var("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();

        let schema_dirs = data_dirs
            .iter()
            .map(|d| d.join("glib-2.0/schemas"))
            .collect();

        let config_root = var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| var("HOME").map(|h| PathBuf::from(h).join(".config")));

        Self {
            env,
            os_release_path,
            kernel_release_path,
            app_dirs,
            path_dirs,
            schema_dirs,
            config_root,
        }
    }
}

/// Result status of one discovery run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanStatus {
    Completed,
    Cancelled,
}

/// Stage timings, for the handoff report's cold/warm measurements.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanTimings {
    pub distribution_micros: u64,
    pub session_micros: u64,
    pub applications_micros: u64,
    pub backends_micros: u64,
    pub total_micros: u64,
}

/// The typed snapshot suggested by 03_ENVIRONMENT_DISCOVERY.md §4 (plus the
/// informational kernel/context facts the plan asks to keep separate from
/// distribution identity).
#[derive(Debug, Clone, PartialEq)]
pub struct EnvironmentSnapshot {
    pub distro: DistributionEvidence,
    pub kernel: Option<KernelInfo>,
    pub execution_context: ExecutionContext,
    pub session: SessionEvidence,
    pub installed_apps: Vec<InstalledApp>,
    pub available_backends: Vec<BackendCapability>,
    pub problems: Vec<DetectionProblem>,
}

impl EnvironmentSnapshot {
    /// The grounded sidebar-footer summary: `Arch Linux · Hyprland ·
    /// Wayland`. Undetected parts are omitted; if nothing is known the
    /// result says so honestly instead of pretending.
    pub fn summary(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(name) = self.distro.display_name() {
            parts.push(name);
        }
        if let Some(name) = self.session.desktop_display_name() {
            parts.push(name.to_string());
        }
        if let Some(protocol) = self.session.protocol_display_name() {
            parts.push(protocol.to_string());
        }
        if parts.is_empty() {
            "Environment unknown".to_string()
        } else {
            parts.join(" · ")
        }
    }
}

/// An incremental discovery run: stages execute one at a time so callers
/// (the UI adapter) can show truthful partial results between them. The
/// one-shot [`discover`] drives the same stages in a loop — a single
/// implementation, two consumption patterns.
#[derive(Debug, Default)]
pub struct DiscoveryRun {
    distro: Option<DistributionEvidence>,
    kernel: Option<KernelInfo>,
    execution_context: ExecutionContext,
    session: Option<SessionEvidence>,
    installed_apps: Vec<InstalledApp>,
    available_backends: Vec<BackendCapability>,
    problems: Vec<DetectionProblem>,
    timings: ScanTimings,
    next_stage: Option<ScanStage>,
    started: Option<Instant>,
}

impl DiscoveryRun {
    pub fn new() -> Self {
        Self {
            next_stage: Some(ScanStage::Distribution),
            started: Some(Instant::now()),
            ..Self::default()
        }
    }

    /// The next stage to run, or `None` when the run is complete.
    pub fn next_stage(&self) -> Option<ScanStage> {
        self.next_stage
    }

    /// Snapshot of everything discovered so far.
    pub fn snapshot(&self) -> EnvironmentSnapshot {
        EnvironmentSnapshot {
            distro: self.distro.clone().unwrap_or_default(),
            kernel: self.kernel.clone(),
            execution_context: self.execution_context.clone(),
            session: self.session.clone().unwrap_or_default(),
            installed_apps: self.installed_apps.clone(),
            available_backends: self.available_backends.clone(),
            problems: self.problems.clone(),
        }
    }

    /// Run exactly one stage. Returns [`ScanStatus::Cancelled`] when the
    /// control flag was set (checked after the stage); the caller may then
    /// stop or continue later — a cancelled run keeps the stages it finished.
    pub fn run_next_stage(
        &mut self,
        sources: &DiscoverySources,
        control: &ScanControl,
        progress: &dyn Fn(ScanProgress),
    ) -> ScanStatus {
        let Some(stage) = self.next_stage else {
            return ScanStatus::Completed;
        };
        progress(ScanProgress {
            stage,
            done: 0,
            total: None,
        });
        match stage {
            ScanStage::Distribution => {
                let (distro, kernel, execution_context, micros, problems) =
                    os_release::collect(sources);
                self.distro = Some(distro);
                self.kernel = kernel;
                self.execution_context = execution_context;
                self.timings.distribution_micros = micros;
                self.problems.extend(problems);
                self.next_stage = Some(ScanStage::Session);
            }
            ScanStage::Session => {
                let (session, micros) = session::detect(&sources.env);
                self.session = Some(session);
                self.timings.session_micros = micros;
                self.next_stage = Some(ScanStage::Applications);
            }
            ScanStage::Applications => {
                let current_desktops: Vec<String> = sources
                    .env
                    .get("XDG_CURRENT_DESKTOP")
                    .map(|raw| raw.split(':').map(str::to_string).collect())
                    .unwrap_or_default();
                let (mut installed_apps, micros, problems) = apps::scan_applications(
                    &sources.app_dirs,
                    &sources.path_dirs,
                    &current_desktops,
                    control,
                    progress,
                );
                installed_apps.sort_by(|a, b| a.id.cmp(&b.id).then_with(|| a.name.cmp(&b.name)));
                self.installed_apps = installed_apps;
                self.timings.applications_micros = micros;
                self.problems.extend(problems);
                self.next_stage = Some(ScanStage::Backends);
            }
            ScanStage::Backends => {
                let started = Instant::now();
                let (available_backends, problems) = backends::probe_all(sources);
                self.available_backends = available_backends;
                self.timings.backends_micros = started.elapsed().as_micros() as u64;
                self.problems.extend(problems);
                self.next_stage = None;
                if let Some(started) = self.started.take() {
                    self.timings.total_micros = started.elapsed().as_micros() as u64;
                }
                log::info(
                    "core::discovery",
                    "environment discovery completed",
                    &[
                        ("apps", self.installed_apps.len().to_string()),
                        ("backends", self.available_backends.len().to_string()),
                        ("problems", self.problems.len().to_string()),
                        ("total_us", self.timings.total_micros.to_string()),
                    ],
                );
            }
        }
        if control.is_cancelled() {
            log::info(
                "core::discovery",
                "environment discovery cancelled",
                &[("stage", stage_debug(stage).to_string())],
            );
            ScanStatus::Cancelled
        } else {
            ScanStatus::Completed
        }
    }
}

fn stage_debug(stage: ScanStage) -> &'static str {
    match stage {
        ScanStage::Distribution => "distribution",
        ScanStage::Session => "session",
        ScanStage::Applications => "applications",
        ScanStage::Backends => "backends",
    }
}

/// One complete discovery run: all stages in order. Bounded, passive and
/// isolated — a failure in any single source becomes a [`DetectionProblem`],
/// never an error return and never a crash. Cancellation between stages
/// yields a partial snapshot with [`ScanStatus::Cancelled`].
pub fn discover(
    sources: &DiscoverySources,
    control: &ScanControl,
    progress: &dyn Fn(ScanProgress),
) -> (EnvironmentSnapshot, ScanStatus, ScanTimings) {
    let mut run = DiscoveryRun::new();
    let mut status = ScanStatus::Completed;
    while run.next_stage().is_some() {
        if run.run_next_stage(sources, control, progress) == ScanStatus::Cancelled {
            status = ScanStatus::Cancelled;
            break;
        }
    }
    (run.snapshot(), status, run.timings)
}

/// Replace a leading `$HOME` with `~` for display/export. Facts that already
/// carry no home path are returned untouched. This is the redaction rule for
/// every user-visible discovery fact.
pub fn redact_path(path: &str, home: Option<&str>) -> String {
    match home {
        Some(home) if !home.is_empty() && path.starts_with(home) => {
            format!("~{}", &path[home.len()..])
        }
        _ => path.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_joins_known_parts_and_omits_unknown() {
        let mut snapshot = demo_fixtures::arch_hyprland_wayland();
        assert_eq!(snapshot.summary(), "Arch Linux · Hyprland · Wayland");

        snapshot.session.desktop = DesktopIdentity::Unknown;
        assert_eq!(snapshot.summary(), "Arch Linux · Wayland");

        snapshot.session.protocol = Protocol::Unknown;
        snapshot.distro = DistributionEvidence::missing();
        assert_eq!(snapshot.summary(), "Environment unknown");
    }

    #[test]
    fn redact_path_replaces_home_prefix_only() {
        let home = Some("/home/baris");
        assert_eq!(
            redact_path("/home/baris/.config/alacritty", home),
            "~/.config/alacritty"
        );
        assert_eq!(
            redact_path("/usr/share/applications/x.desktop", home),
            "/usr/share/applications/x.desktop"
        );
        assert_eq!(redact_path("/home/baris", home), "~");
        assert_eq!(redact_path("/home/other/file", home), "/home/other/file");
    }

    #[test]
    fn scan_control_roundtrip() {
        let control = ScanControl::new();
        assert!(!control.is_cancelled());
        control.cancel();
        assert!(control.is_cancelled());
    }
}
