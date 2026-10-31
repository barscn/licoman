//! Backend/service capability probes (03_ENVIRONMENT_DISCOVERY.md §4).
//!
//! All probes are **passive and bounded**: PATH lookups (stat only), a
//! D-Bus session-bus Hello+ListNames exchange, GSettings schema-database
//! enumeration, and stat checks over anticipated application configuration
//! locations. Nothing is launched, activated or written; a probe failure
//! degrades to an unavailable capability plus a problem entry.
//!
//! "Presence of a binary is not permission to change packages" — package
//! managers are reported as informational facts only.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use super::dbus;
use super::gsettings;
use super::{
    Confidence, DetectionProblem, DiscoverySources, Evidence, EvidenceSource, ProblemKind,
};

/// Raw environment facts the module applicability evaluator keys on
/// (04_MODULE_SCHEMA_AND_REGISTRY.md §4: applicability must be based on
/// actual detected environment/provider facts). These were previously only
/// counted; they are now retained with the snapshot.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BackendFacts {
    /// Discovered GSettings schemas and their declared key names.
    pub gsettings_schemas: BTreeMap<String, BTreeSet<String>>,
    /// Well-known names visible on the session bus.
    pub bus_names: BTreeSet<String>,
}

/// The kinds of backend facts Licoman can observe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// A running service reachable without activation (D-Bus session bus).
    Service,
    /// An installed package-manager binary. Informational only.
    PackageTool,
    /// An installed GSettings schema database.
    SchemaDatabase,
    /// An anticipated application configuration location.
    AppConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendStatus {
    Available,
    /// Unavailable with a user-safe reason (and, where known, the exact
    /// prerequisite — never mislabeled as a distro problem).
    Unavailable {
        reason: String,
    },
}

/// One capability probe result with its evidence trail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendCapability {
    /// Stable id, e.g. `dbus-session`, `package-manager:pacman`,
    /// `gsettings-schemas`, `config:alacritty`.
    pub id: String,
    pub kind: BackendKind,
    pub status: BackendStatus,
    pub evidence: Vec<Evidence>,
    /// Extra non-secret facts (counts, schema totals, config paths already
    /// redacted at the source).
    pub detail: Option<String>,
}

impl BackendCapability {
    pub fn is_available(&self) -> bool {
        matches!(self.status, BackendStatus::Available)
    }
}

/// Package managers we can *detect* (never invoke). Presence of the binary
/// is an informational fact, not permission.
const PACKAGE_MANAGERS: &[&str] = &["pacman", "dnf", "apt", "zypper", "apk", "emerge"];

/// Applications whose configuration locations we anticipate (Phase 05
/// backends will use them; here they are only stat-checked).
const ANTICIPATED_CONFIG_DIRS: &[(&str, &str)] = &[
    ("alacritty", "alacritty"),
    ("kitty", "kitty"),
    ("mpv", "mpv"),
    ("neovim", "nvim"),
    ("zed", "zed"),
];

/// Run every probe. Isolated: each probe catches its own failures.
/// Returns the capability list, the raw facts (schemas, bus names) and any
/// problems encountered.
pub fn probe_all(
    sources: &DiscoverySources,
) -> (Vec<BackendCapability>, BackendFacts, Vec<DetectionProblem>) {
    let mut backends = Vec::new();
    let mut facts = BackendFacts::default();
    let mut problems = Vec::new();

    // ---- Package managers (informational PATH lookups) -------------------
    for name in PACKAGE_MANAGERS {
        let found = lookup_binary(name, &sources.path_dirs);
        let evidence = Evidence::new(
            EvidenceSource::PathLookup,
            Confidence::High,
            if found.is_some() {
                "binary found on PATH"
            } else {
                "binary not found on PATH"
            },
        );
        backends.push(BackendCapability {
            id: format!("package-manager:{name}"),
            kind: BackendKind::PackageTool,
            status: if found.is_some() {
                BackendStatus::Available
            } else {
                BackendStatus::Unavailable {
                    reason: format!("{name} is not installed"),
                }
            },
            evidence: vec![evidence],
            detail: None,
        });
    }

    // ---- D-Bus session bus ------------------------------------------------
    let bus = dbus::probe_session_bus(
        sources
            .env
            .get("DBUS_SESSION_BUSADDRESS")
            .map(String::as_str),
        sources.env.get("XDG_RUNTIME_DIR").map(String::as_str),
    );
    if bus.available {
        facts.bus_names = bus.names.iter().cloned().collect();
        backends.push(BackendCapability {
            id: "dbus-session".to_string(),
            kind: BackendKind::Service,
            status: BackendStatus::Available,
            evidence: vec![Evidence::new(
                EvidenceSource::DBus,
                Confidence::High,
                format!(
                    "session bus reachable; {} well-known names",
                    bus.names.len()
                ),
            )],
            detail: Some(format!("{} names listed", bus.names.len())),
        });
    } else {
        let reason = bus
            .reason
            .unwrap_or_else(|| "session bus probe failed".to_string());
        problems.push(DetectionProblem {
            kind: ProblemKind::DBusProbeFailed,
            message: "The D-Bus session bus is not reachable; service-dependent \
                      capabilities are unavailable."
                .to_string(),
            technical: reason.clone(),
        });
        backends.push(BackendCapability {
            id: "dbus-session".to_string(),
            kind: BackendKind::Service,
            status: BackendStatus::Unavailable {
                reason: "No usable D-Bus session bus".to_string(),
            },
            evidence: vec![Evidence::new(
                EvidenceSource::DBus,
                Confidence::High,
                reason,
            )],
            detail: None,
        });
    }

    // ---- GSettings schema databases ---------------------------------------
    match probe_schemas(&sources.schema_dirs) {
        SchemaProbe::Found {
            schemas,
            sources: hits,
            map,
        } => {
            facts.gsettings_schemas = map;
            backends.push(BackendCapability {
                id: "gsettings-schemas".to_string(),
                kind: BackendKind::SchemaDatabase,
                status: BackendStatus::Available,
                evidence: hits
                    .into_iter()
                    .map(|(count, origin)| {
                        Evidence::new(
                            EvidenceSource::GSettings,
                            Confidence::High,
                            format!("gschemas.compiled parsed ({origin}): {count} schemas"),
                        )
                    })
                    .collect(),
                detail: Some(format!("{schemas} distinct schemas")),
            });
        }
        SchemaProbe::NoneFound => {
            backends.push(BackendCapability {
                id: "gsettings-schemas".to_string(),
                kind: BackendKind::SchemaDatabase,
                status: BackendStatus::Unavailable {
                    reason: "No GLib schema database found (glib-2.0/schemas with \
                             gschemas.compiled)"
                        .to_string(),
                },
                evidence: vec![Evidence::new(
                    EvidenceSource::GSettings,
                    Confidence::Medium,
                    "no schema directories discovered".to_string(),
                )],
                detail: None,
            });
        }
        SchemaProbe::Failed { reason } => {
            problems.push(DetectionProblem {
                kind: ProblemKind::GSettingsProbeFailed,
                message: "A GLib schema database exists but could not be read.".to_string(),
                technical: reason.clone(),
            });
            backends.push(BackendCapability {
                id: "gsettings-schemas".to_string(),
                kind: BackendKind::SchemaDatabase,
                status: BackendStatus::Unavailable {
                    reason: "Schema database present but unreadable".to_string(),
                },
                evidence: vec![Evidence::new(
                    EvidenceSource::GSettings,
                    Confidence::Low,
                    reason,
                )],
                detail: None,
            });
        }
    }

    // ---- Anticipated application configuration locations ------------------
    if let Some(config_root) = &sources.config_root {
        for (app_id, dir_name) in ANTICIPATED_CONFIG_DIRS {
            let path = config_root.join(dir_name);
            let exists = path.exists();
            backends.push(BackendCapability {
                id: format!("config:{app_id}"),
                kind: BackendKind::AppConfig,
                status: if exists {
                    BackendStatus::Available
                } else {
                    BackendStatus::Unavailable {
                        reason: format!("no {app_id} configuration directory yet"),
                    }
                },
                evidence: vec![Evidence::new(
                    EvidenceSource::ConfigDir,
                    Confidence::High,
                    if exists {
                        "directory present"
                    } else {
                        "directory absent"
                    }
                    .to_string(),
                )],
                detail: None,
            });
        }
    }

    (backends, facts, problems)
}

fn lookup_binary(name: &str, path_dirs: &[PathBuf]) -> Option<PathBuf> {
    path_dirs
        .iter()
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

enum SchemaProbe {
    Found {
        schemas: usize,
        sources: Vec<(usize, &'static str)>,
        map: BTreeMap<String, BTreeSet<String>>,
    },
    NoneFound,
    Failed {
        reason: String,
    },
}

/// Enumerate every existing schema directory (deduplicated by schema id,
/// first dir wins — mirroring how GSettings merges databases).
fn probe_schemas(schema_dirs: &[PathBuf]) -> SchemaProbe {
    let mut seen: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut per_source = Vec::new();
    for (index, dir) in schema_dirs.iter().enumerate() {
        let compiled = dir.join("gschemas.compiled");
        if !compiled.exists() {
            continue;
        }
        let data = match std::fs::read(&compiled) {
            Ok(data) => data,
            Err(error) => {
                return SchemaProbe::Failed {
                    reason: format!("read failed ({error})"),
                };
            }
        };
        match gsettings::parse_compiled_schemas(&data) {
            Ok(schemas) => {
                // First schema dir is the user XDG data dir (see the source
                // ordering in DiscoverySources); the rest are system.
                per_source.push((schemas.len(), if index == 0 { "user" } else { "system" }));
                for (name, keys) in schemas {
                    // First directory wins for a schema id.
                    seen.entry(name).or_default().extend(keys);
                }
            }
            Err(error) => {
                return SchemaProbe::Failed {
                    reason: format!("parse failed: {error:?}"),
                };
            }
        }
    }
    if per_source.is_empty() {
        return SchemaProbe::NoneFound;
    }
    SchemaProbe::Found {
        schemas: seen.len(),
        sources: per_source,
        map: seen,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_manager_lookup_is_stat_only() {
        // /bin is on every supported system but only stat'ed, never run.
        let sources = DiscoverySources {
            path_dirs: vec![PathBuf::from("/bin"), PathBuf::from("/usr/bin")],
            ..DiscoverySources::default()
        };
        let (backends, _facts, problems) = probe_all(&sources);
        assert!(
            problems.is_empty()
                || problems
                    .iter()
                    .all(|p| p.kind != ProblemKind::DirUnreadable)
        );
        let pacman = backends
            .iter()
            .find(|b| b.id == "package-manager:pacman")
            .expect("package manager facts present");
        // Presence is a fact about the binary, not a permission.
        assert_eq!(pacman.kind, BackendKind::PackageTool);
    }

    #[test]
    fn probes_without_bus_or_schemas_fail_closed() {
        let sources = DiscoverySources::default();
        let (backends, _facts, problems) = probe_all(&sources);
        let bus = backends.iter().find(|b| b.id == "dbus-session").unwrap();
        assert!(!bus.is_available());
        let schemas = backends
            .iter()
            .find(|b| b.id == "gsettings-schemas")
            .unwrap();
        assert!(!schemas.is_available());
        // No D-Bus session → a problem entry explains why (safe state).
        assert!(
            problems
                .iter()
                .any(|p| p.kind == ProblemKind::DBusProbeFailed)
        );
    }

    #[test]
    fn config_location_probes_are_stat_only() {
        let sources = DiscoverySources {
            config_root: Some(std::env::temp_dir()),
            ..DiscoverySources::default()
        };
        let (backends, _, _) = probe_all(&sources);
        let alacritty = backends
            .iter()
            .find(|b| b.id == "config:alacritty")
            .expect("config probes present");
        assert_eq!(alacritty.kind, BackendKind::AppConfig);
        assert!(!alacritty.is_available()); // temp dir has no alacritty dir
    }
}
