//! Installed-application enumeration (03_ENVIRONMENT_DISCOVERY.md §3).
//!
//! Parses XDG `.desktop` entries from user/system application directories,
//! respecting data-directory precedence and desktop-entry semantics
//! (`Hidden`, `NoDisplay`, `TryExec`, `OnlyShowIn`/`NotShowIn`). Dedup key is
//! the desktop-file id (the file name), not the display name.
//!
//! Hard rules:
//! - `Exec` lines are **never executed** and never stored — only their
//!   existence is checked (the spec requires `Exec` for applications).
//! - `TryExec` is resolved with a `stat`, never a spawn.
//! - Executable paths and icon values are stored as unvalidated metadata and
//!   are never treated as settings targets.
//! - A broken or malicious entry becomes a [`DetectionProblem`]; it cannot
//!   prevent the scan of other entries or the launch of Licoman.
//! - Only this user's XDG directories and system data dirs are scanned; no
//!   other user's home directory is ever traversed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::{
    Confidence, DetectionProblem, Evidence, EvidenceSource, ProblemKind, ScanControl, ScanProgress,
    ScanStage,
};
use crate::log;

/// Where an application was discovered. Flatpak export directories are
/// scanned like any other application directory but keep their own origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallOrigin {
    User,
    System,
    FlatpakUser,
    FlatpakSystem,
}

impl InstallOrigin {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::System => "system",
            Self::FlatpakUser => "flatpak-user",
            Self::FlatpakSystem => "flatpak-system",
        }
    }

    /// Human label for evidence and diagnostics lines.
    pub fn label(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::System => "system",
            Self::FlatpakUser => "flatpak (user)",
            Self::FlatpakSystem => "flatpak (system)",
        }
    }
}

/// One application directory in XDG precedence order (first entry wins).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppScanDir {
    pub path: PathBuf,
    pub origin: InstallOrigin,
}

/// One discovered installed application. `Installed` is a fact; whether a
/// Licoman module exists is a *separate* condition (see `matching`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledApp {
    /// Desktop-file identity, e.g. `org.gnome.Nautilus.desktop` — stable
    /// across data-directory shadowing.
    pub id: String,
    pub name: String,
    pub generic_name: Option<String>,
    pub comment: Option<String>,
    /// Icon name or path, as declared. Never validated, never used to locate
    /// anything.
    pub icon: Option<String>,
    pub origin: InstallOrigin,
    /// Install-source evidence: the winning desktop file's location.
    pub source_path: PathBuf,
    /// `NoDisplay=true`: exists but hidden from menus. Kept for the
    /// all-applications view, excluded from primary lists.
    pub no_display: bool,
    /// `TryExec` resolution result: true when absent or when the referenced
    /// binary exists (stat only).
    pub try_exec_found: bool,
    /// `OnlyShowIn`/`NotShowIn` excludes the current desktop.
    pub restricted_from_current: bool,
    pub wm_class: Option<String>,
    /// Flatpak application id when the entry declares `X-Flatpak`.
    pub flatpak_id: Option<String>,
    pub evidence: Vec<Evidence>,
}

/// Maximum accepted desktop-file size. Larger files are skipped with a
/// problem entry — a 1 MiB text file is not a legitimate launcher.
const MAX_DESKTOP_FILE_BYTES: u64 = 1024 * 1024;

/// Why a parsed entry is not usable as an application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryError {
    /// `Type` is present but not `Application` (Links/Directories are not
    /// applications; not an error condition for the scan).
    NotAnApplication,
    /// Structural problem; `detail` is user-safe and content-free.
    Invalid { detail: String },
}

/// Pure desktop-entry parser over the `[Desktop Entry]` group. Locale-keyed
/// variants (`Name[de]`) are ignored in favor of the unlocalized key;
/// malformed lines are skipped (counted, not fatal); required fields are
/// validated per the spec.
pub fn parse_desktop_entry(text: &str) -> Result<ParsedEntry, EntryError> {
    let mut fields: BTreeMap<String, String> = BTreeMap::new();
    let mut in_main_group = false;
    let mut saw_main_group = false;
    let mut skipped_lines = 0usize;

    for raw_line in text.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if let Some(group) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_main_group = group == "Desktop Entry";
            saw_main_group |= in_main_group;
            continue;
        }
        if !in_main_group {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            skipped_lines += 1;
            continue;
        };
        let key = key.trim();
        // Ignore locale-suffixed variants (`Name[de]=…`): we display the
        // unlocalized key.
        if key.is_empty() || key.contains('[') {
            continue;
        }
        fields
            .entry(key.to_string())
            .or_insert_with(|| unescape(value.trim()));
    }

    if !saw_main_group {
        return Err(EntryError::Invalid {
            detail: "no [Desktop Entry] group".to_string(),
        });
    }
    if skipped_lines > 0 {
        return Err(EntryError::Invalid {
            detail: format!("{skipped_lines} malformed line(s)"),
        });
    }

    match fields.get("Type").map(String::as_str) {
        None => {
            return Err(EntryError::Invalid {
                detail: "missing Type".to_string(),
            });
        }
        Some("Application") => {}
        Some(_) => return Err(EntryError::NotAnApplication),
    }
    let Some(name) = fields.get("Name").filter(|n| !n.is_empty()) else {
        return Err(EntryError::Invalid {
            detail: "missing Name".to_string(),
        });
    };
    if !fields.contains_key("Exec") {
        return Err(EntryError::Invalid {
            detail: "missing Exec".to_string(),
        });
    }

    Ok(ParsedEntry {
        name: name.clone(),
        generic_name: fields.get("GenericName").filter(|v| !v.is_empty()).cloned(),
        comment: fields.get("Comment").filter(|v| !v.is_empty()).cloned(),
        icon: fields.get("Icon").filter(|v| !v.is_empty()).cloned(),
        hidden: fields.get("Hidden").map(|v| v == "true").unwrap_or(false),
        no_display: fields
            .get("NoDisplay")
            .map(|v| v == "true")
            .unwrap_or(false),
        try_exec: fields.get("TryExec").filter(|v| !v.is_empty()).cloned(),
        only_show_in: split_list(fields.get("OnlyShowIn")),
        not_show_in: split_list(fields.get("NotShowIn")),
        wm_class: fields
            .get("StartupWMClass")
            .filter(|v| !v.is_empty())
            .cloned(),
        flatpak_id: fields.get("X-Flatpak").filter(|v| !v.is_empty()).cloned(),
    })
}

/// Minimal unescape for display strings (`\s`, `\n`, `\t`, `\r`, `\\`).
fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('s') => out.push(' '),
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn split_list(value: Option<&String>) -> Vec<String> {
    value
        .map(|v| {
            v.split(';')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The fields Licoman reads from a desktop entry. `Exec` is deliberately
/// absent — it is validated for existence during parsing and discarded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedEntry {
    pub name: String,
    pub generic_name: Option<String>,
    pub comment: Option<String>,
    pub icon: Option<String>,
    pub hidden: bool,
    pub no_display: bool,
    pub try_exec: Option<String>,
    pub only_show_in: Vec<String>,
    pub not_show_in: Vec<String>,
    pub wm_class: Option<String>,
    pub flatpak_id: Option<String>,
}

/// Pure restriction evaluation against the current desktop indicator list
/// (the raw `XDG_CURRENT_DESKTOP` elements, lowercased).
pub fn restricted_from_current(entry: &ParsedEntry, current_desktops: &[String]) -> bool {
    let current: Vec<String> = current_desktops
        .iter()
        .map(|d| d.to_ascii_lowercase())
        .collect();
    if !entry.only_show_in.is_empty()
        && !entry
            .only_show_in
            .iter()
            .any(|d| current.contains(&d.to_ascii_lowercase()))
    {
        return true;
    }
    entry
        .not_show_in
        .iter()
        .any(|d| current.contains(&d.to_ascii_lowercase()))
}

/// Resolve a `TryExec` value against the PATH snapshot with a `stat` —
/// never a spawn.
fn try_exec_found(try_exec: &str, path_dirs: &[PathBuf]) -> bool {
    let candidate = Path::new(try_exec);
    if candidate.is_absolute() {
        return candidate.is_file();
    }
    path_dirs.iter().any(|dir| dir.join(candidate).is_file())
}

/// Scan the application directories in precedence order. Returns the
/// discovered applications, elapsed micros and isolated problems.
pub fn scan_applications(
    app_dirs: &[AppScanDir],
    path_dirs: &[PathBuf],
    current_desktops: &[String],
    control: &ScanControl,
    progress: &dyn Fn(ScanProgress),
) -> (Vec<InstalledApp>, u64, Vec<DetectionProblem>) {
    let started = Instant::now();
    let mut problems = Vec::new();
    let mut by_id: BTreeMap<String, InstalledApp> = BTreeMap::new();
    let mut skipped_hidden = 0usize;
    let mut shadowed = 0usize;

    for dir in app_dirs {
        if control.is_cancelled() {
            break;
        }
        let entries = match std::fs::read_dir(&dir.path) {
            Ok(entries) => entries,
            Err(_) => {
                // A missing application directory is normal (many systems
                // have no Flatpak exports); only record it once it exists
                // but cannot be read.
                if dir.path.exists() {
                    problems.push(DetectionProblem {
                        kind: ProblemKind::DirUnreadable,
                        message: "An application directory could not be read; some applications may be missing."
                            .to_string(),
                        technical: format!("read_dir failed: {}", dir.path.display()),
                    });
                }
                continue;
            }
        };

        let mut files: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "desktop"))
            .collect();
        files.sort();
        let total = files.len();
        progress(ScanProgress {
            stage: ScanStage::Applications,
            done: 0,
            total: Some(total),
        });

        for (done, path) in files.iter().enumerate() {
            if control.is_cancelled() {
                break;
            }
            progress(ScanProgress {
                stage: ScanStage::Applications,
                done: done + 1,
                total: Some(total),
            });

            // Only regular files (follows symlinks); this also rejects
            // FIFOs/devices planted in an applications directory.
            let meta = match std::fs::metadata(path) {
                Ok(meta) => meta,
                Err(_) => continue,
            };
            if !meta.is_file() {
                continue;
            }
            if meta.len() > MAX_DESKTOP_FILE_BYTES {
                problems.push(DetectionProblem {
                    kind: ProblemKind::DesktopEntrySkipped,
                    message: "An unusually large application entry was skipped.".to_string(),
                    technical: format!("file exceeds size cap: {}", path.display()),
                });
                continue;
            }

            let text = match std::fs::read_to_string(path) {
                Ok(text) => text,
                Err(_) => {
                    problems.push(DetectionProblem {
                        kind: ProblemKind::DesktopEntryInvalid,
                        message: "An application entry could not be read or is not valid UTF-8."
                            .to_string(),
                        technical: format!("read failed: {}", path.display()),
                    });
                    continue;
                }
            };

            let file_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string();
            let entry = match parse_desktop_entry(&text) {
                Ok(entry) => entry,
                Err(EntryError::NotAnApplication) => continue,
                Err(EntryError::Invalid { detail }) => {
                    problems.push(DetectionProblem {
                        kind: ProblemKind::DesktopEntryInvalid,
                        message: format!(
                            "The application entry {file_name} is invalid and was skipped: {detail}."
                        ),
                        technical: format!("parse failed ({detail}): {}", path.display()),
                    });
                    continue;
                }
            };

            if entry.hidden {
                // Spec: Hidden means "the user deleted this application".
                skipped_hidden += 1;
                continue;
            }

            // XDG precedence: first data directory in search order wins.
            if by_id.contains_key(&file_name) {
                shadowed += 1;
                continue;
            }

            let try_exec_ok = match &entry.try_exec {
                Some(try_exec) => {
                    let found = try_exec_found(try_exec, path_dirs);
                    if !found {
                        // Spec: an entry whose TryExec is missing should be
                        // ignored; we keep the explanation instead.
                        problems.push(DetectionProblem {
                            kind: ProblemKind::DesktopEntrySkipped,
                            message: format!(
                                "The application {file_name} is skipped because its TryExec binary was not found."
                            ),
                            technical: format!("TryExec not found: {}", path.display()),
                        });
                        continue;
                    }
                    found
                }
                None => true,
            };

            let evidence = vec![Evidence::new(
                EvidenceSource::DesktopEntry,
                Confidence::High,
                format!("desktop entry {} ({})", file_name, dir.origin.as_str()),
            )];
            let restricted = restricted_from_current(&entry, current_desktops);

            by_id.insert(
                file_name.clone(),
                InstalledApp {
                    id: file_name,
                    name: entry.name,
                    generic_name: entry.generic_name,
                    comment: entry.comment,
                    icon: entry.icon,
                    origin: dir.origin,
                    source_path: path.clone(),
                    no_display: entry.no_display,
                    try_exec_found: try_exec_ok,
                    restricted_from_current: restricted,
                    wm_class: entry.wm_class,
                    flatpak_id: entry.flatpak_id,
                    evidence,
                },
            );
        }
    }

    let apps: Vec<InstalledApp> = by_id.into_values().collect();
    log::info(
        "core::discovery",
        "application scan finished",
        &[
            ("apps", apps.len().to_string()),
            ("hidden", skipped_hidden.to_string()),
            ("shadowed", shadowed.to_string()),
            ("problems", problems.len().to_string()),
        ],
    );
    (apps, started.elapsed().as_micros() as u64, problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(text: &str) -> ParsedEntry {
        parse_desktop_entry(text).expect("entry parses")
    }

    const VALID: &str = "\
[Desktop Entry]
Type=Application
Name=Test App
GenericName=Tester
Comment=A test application
Exec=/usr/bin/test-app --flag
Icon=test-app
TryExec=test-app
StartupWMClass=TestApp
";

    /// Same entry without TryExec, for scan tests (whose PATH is empty, so
    /// a TryExec would — correctly — cause a skip).
    const SCAN_VALID: &str = "\
[Desktop Entry]
Type=Application
Name=Test App
Exec=/usr/bin/test-app
";

    #[test]
    fn parses_valid_entry_fields() {
        let entry = parse_ok(VALID);
        assert_eq!(entry.name, "Test App");
        assert_eq!(entry.generic_name.as_deref(), Some("Tester"));
        assert_eq!(entry.comment.as_deref(), Some("A test application"));
        assert_eq!(entry.icon.as_deref(), Some("test-app"));
        assert_eq!(entry.wm_class.as_deref(), Some("TestApp"));
        assert!(!entry.hidden);
        assert!(!entry.no_display);
    }

    #[test]
    fn exec_is_never_stored() {
        let entry = parse_ok(VALID);
        let rendered = format!("{entry:?}");
        assert!(!rendered.contains("/usr/bin/test-app"));
        assert!(!rendered.contains("--flag"));
    }

    #[test]
    fn locale_variants_are_ignored_for_unlocalized_key() {
        let entry = parse_ok(
            "[Desktop Entry]\nType=Application\nName=Only Name\nName[de]=Nur Name\nExec=x\n",
        );
        assert_eq!(entry.name, "Only Name");
    }

    #[test]
    fn hidden_no_display_and_restrictions_are_read() {
        let entry = parse_ok(
            "[Desktop Entry]\nType=Application\nName=X\nExec=x\nHidden=true\nNoDisplay=true\nOnlyShowIn=GNOME;KDE;\nNotShowIn=XFCE;\n",
        );
        assert!(entry.hidden);
        assert!(entry.no_display);
        assert_eq!(entry.only_show_in, vec!["GNOME", "KDE"]);
        assert_eq!(entry.not_show_in, vec!["XFCE"]);
    }

    #[test]
    fn unescape_handles_spec_sequences() {
        let entry = parse_ok("[Desktop Entry]\nType=Application\nName=A\\\\B\\sC\\nD\nExec=x\n");
        assert_eq!(entry.name, "A\\B C\nD");
    }

    #[test]
    fn invalid_entries_fail_with_content_free_details() {
        for (text, expected) in [
            ("", "no [Desktop Entry] group"),
            ("[Desktop Entry]\nName=X\nExec=x\n", "missing Type"),
            (
                "[Desktop Entry]\nType=Application\nExec=x\n",
                "missing Name",
            ),
            (
                "[Desktop Entry]\nType=Application\nName=X\n",
                "missing Exec",
            ),
            (
                "[Desktop Entry]\nType=Application\nName=X\nbroken line\nExec=x\n",
                "malformed line",
            ),
        ] {
            let err = parse_desktop_entry(text).unwrap_err();
            match err {
                EntryError::Invalid { detail } => {
                    assert!(detail.contains(expected), "{text:?} -> {detail}")
                }
                other => panic!("expected Invalid for {text:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn links_and_directories_are_not_applications() {
        for text in [
            "[Desktop Entry]\nType=Link\nName=X\nURL=http://example.com\n",
            "[Desktop Entry]\nType=Directory\nName=X\n",
        ] {
            assert_eq!(parse_desktop_entry(text), Err(EntryError::NotAnApplication));
        }
    }

    #[test]
    fn restriction_logic_matches_case_insensitively() {
        let entry =
            parse_ok("[Desktop Entry]\nType=Application\nName=X\nExec=x\nOnlyShowIn=GNOME;\n");
        let hyprland = vec!["Hyprland".to_string()];
        let gnome = vec!["GNOME".to_string()];
        assert!(restricted_from_current(&entry, &hyprland));
        assert!(!restricted_from_current(&entry, &gnome));

        let excluded =
            parse_ok("[Desktop Entry]\nType=Application\nName=X\nExec=x\nNotShowIn=Hyprland;\n");
        assert!(restricted_from_current(&excluded, &hyprland));
    }

    #[test]
    fn try_exec_resolution_never_spawns() {
        // An absolute path to something that is not a file.
        assert!(!try_exec_found("/definitely/not/a/binary", &[]));

        // A real file resolves; a PATH lookup finds it from a search dir.
        let tmp = std::env::temp_dir().join(format!("licoman-tryexec-{}", std::process::id()));
        std::fs::write(&tmp, b"").unwrap();
        assert!(try_exec_found(tmp.to_str().unwrap(), &[]));
        let parent = tmp.parent().unwrap().to_path_buf();
        let name = tmp.file_name().unwrap().to_str().unwrap().to_string();
        assert!(try_exec_found(&name, &[parent]));
        std::fs::remove_file(&tmp).ok();
    }

    // ---- Scan behavior (temp-dir based; file fixtures live under
    // tests/fixtures/discovery and are exercised by the testkit tests) ----

    struct ScanDir {
        root: PathBuf,
    }

    impl ScanDir {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir()
                .join(format!("licoman-apps-test-{}-{name}", std::process::id()));
            std::fs::create_dir_all(root.join("applications")).unwrap();
            Self { root }
        }

        fn write(&self, file: &str, content: &str) {
            std::fs::write(self.root.join("applications").join(file), content).unwrap();
        }

        fn app_dir(&self, origin: InstallOrigin) -> AppScanDir {
            AppScanDir {
                path: self.root.join("applications"),
                origin,
            }
        }
    }

    impl Drop for ScanDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).ok();
        }
    }

    fn scan(
        dirs: &[AppScanDir],
        control: &ScanControl,
    ) -> (Vec<InstalledApp>, Vec<DetectionProblem>) {
        let (apps, _, problems) = scan_applications(dirs, &[], &[], control, &|_| {});
        (apps, problems)
    }

    #[test]
    fn scan_skips_broken_entries_and_keeps_others() {
        let dir = ScanDir::new("broken");
        dir.write("good.desktop", SCAN_VALID);
        dir.write("broken.desktop", "[Desktop Entry]\nName=No Type\n");
        // Raw invalid-UTF-8 bytes (a string literal cannot hold them).
        std::fs::write(
            dir.root.join("applications").join("invalid_utf8.desktop"),
            b"[Desktop Entry]\nType=Application\nName=X\nExec=x\n\xff\xfe\n",
        )
        .unwrap();
        let (apps, problems) = scan(&[dir.app_dir(InstallOrigin::User)], &ScanControl::new());
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].id, "good.desktop");
        assert_eq!(problems.len(), 2);
        assert!(
            problems
                .iter()
                .all(|p| p.kind == ProblemKind::DesktopEntryInvalid)
        );
    }

    #[test]
    fn scan_dedups_by_desktop_file_identity_with_precedence() {
        let system = ScanDir::new("dedup-system");
        let user = ScanDir::new("dedup-user");
        system.write(
            "shared.desktop",
            "[Desktop Entry]\nType=Application\nName=System Version\nExec=x\n",
        );
        user.write(
            "shared.desktop",
            "[Desktop Entry]\nType=Application\nName=User Version\nExec=x\n",
        );
        user.write(
            "only-user.desktop",
            "[Desktop Entry]\nType=Application\nName=Only User\nExec=x\n",
        );
        let (apps, _) = scan(
            &[
                user.app_dir(InstallOrigin::User),
                system.app_dir(InstallOrigin::System),
            ],
            &ScanControl::new(),
        );
        assert_eq!(apps.len(), 2);
        let shared = apps.iter().find(|a| a.id == "shared.desktop").unwrap();
        assert_eq!(shared.name, "User Version", "user dir wins");
        assert_eq!(shared.origin, InstallOrigin::User);
    }

    #[test]
    fn scan_hidden_and_tryexec_missing_are_skipped_with_reasons() {
        let dir = ScanDir::new("skips");
        dir.write(
            "hidden.desktop",
            "[Desktop Entry]\nType=Application\nName=Hidden\nExec=x\nHidden=true\n",
        );
        dir.write(
            "tryexec.desktop",
            "[Desktop Entry]\nType=Application\nName=Missing Binary\nExec=x\nTryExec=licoman-no-such-binary-1234\n",
        );
        let (apps, problems) = scan(&[dir.app_dir(InstallOrigin::System)], &ScanControl::new());
        assert!(apps.is_empty());
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].kind, ProblemKind::DesktopEntrySkipped);
        assert!(problems[0].technical.contains("TryExec"));
    }

    #[test]
    fn scan_marks_no_display_and_restrictions() {
        let dir = ScanDir::new("flags");
        dir.write(
            "hidden_utility.desktop",
            "[Desktop Entry]\nType=Application\nName=Utility\nExec=x\nNoDisplay=true\n",
        );
        dir.write(
            "restricted.desktop",
            "[Desktop Entry]\nType=Application\nName=Only GNOME\nExec=x\nOnlyShowIn=GNOME;\n",
        );
        let (apps, _, _) = scan_applications(
            &[dir.app_dir(InstallOrigin::User)],
            &[],
            &["Hyprland".to_string()],
            &ScanControl::new(),
            &|_| {},
        );
        let utility = apps
            .iter()
            .find(|a| a.id == "hidden_utility.desktop")
            .unwrap();
        assert!(utility.no_display);
        let restricted = apps.iter().find(|a| a.id == "restricted.desktop").unwrap();
        assert!(restricted.restricted_from_current);
    }

    #[test]
    fn scan_records_flatpak_origin_and_id() {
        let dir = ScanDir::new("flatpak");
        dir.write(
            "org.example.Editor.desktop",
            "[Desktop Entry]\nType=Application\nName=Editor\nExec=editor\nX-Flatpak=org.example.Editor\n",
        );
        let (apps, _) = scan(
            &[dir.app_dir(InstallOrigin::FlatpakUser)],
            &ScanControl::new(),
        );
        assert_eq!(apps[0].flatpak_id.as_deref(), Some("org.example.Editor"));
        assert_eq!(apps[0].origin, InstallOrigin::FlatpakUser);
    }

    #[test]
    fn scan_is_cancellable_and_reports_partial() {
        let dir = ScanDir::new("cancel");
        for i in 0..50 {
            dir.write(
                &format!("app{i:03}.desktop"),
                &format!("[Desktop Entry]\nType=Application\nName=App {i}\nExec=x\n"),
            );
        }
        let control = ScanControl::new();
        // Cancel from inside the progress callback — deterministically mid-scan.
        let calls = std::cell::Cell::new(0);
        let (apps, _, problems) = scan_applications(
            &[dir.app_dir(InstallOrigin::User)],
            &[],
            &[],
            &control,
            &|progress| {
                calls.set(calls.get() + 1);
                if progress.done >= 5 {
                    control.cancel();
                }
            },
        );
        assert!(apps.len() < 50, "cancelled scan must be partial");
        assert!(calls.get() >= 5);
        assert!(problems.is_empty(), "cancellation is not a problem");
        assert_eq!(apps[0].id, "app000.desktop");
    }

    #[test]
    fn scan_ignores_non_regular_files_and_missing_dirs() {
        let dir = ScanDir::new("nonregular");
        dir.write("real.desktop", SCAN_VALID);
        // A directory named *.desktop must be ignored like any other
        // non-regular file (the scan never blocks on FIFO-like entries).
        std::fs::create_dir_all(dir.root.join("applications").join("pipe.desktop")).ok();
        let mut dirs = vec![dir.app_dir(InstallOrigin::User)];
        dirs.push(AppScanDir {
            path: dir.root.join("does-not-exist"),
            origin: InstallOrigin::System,
        });
        let (apps, problems) = scan(&dirs, &ScanControl::new());
        assert_eq!(apps.len(), 1);
        // A missing directory is normal, not a problem.
        assert!(problems.is_empty());
    }
}
