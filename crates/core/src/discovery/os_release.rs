//! Distribution and system facts (03_ENVIRONMENT_DISCOVERY.md §1).
//!
//! `/etc/os-release` is parsed according to the standard: `KEY=value` pairs,
//! optional single/double quotes, comments only on their own lines. Missing,
//! malformed and future fields are tolerated — matching happens on `ID` /
//! `ID_LIKE`, never on translated pretty names, and a kernel version is
//! never mistaken for distribution identity.

use std::collections::BTreeMap;
use std::time::Instant;

use super::{
    Confidence, DetectionProblem, DiscoverySources, Evidence, EvidenceSource, ProblemKind,
};
use crate::log;

/// What the os-release source looked like. Fail-closed: anything other than
/// `Found` means "distribution unknown", and the app stays usable.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum OsReleaseStatus {
    #[default]
    Missing,
    Found,
    Malformed,
}

/// Distribution identity with the evidence trail attached.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DistributionEvidence {
    pub id: Option<String>,
    pub id_like: Vec<String>,
    pub name: Option<String>,
    pub version_id: Option<String>,
    pub pretty_name: Option<String>,
    pub status: OsReleaseStatus,
    pub evidence: Vec<Evidence>,
}

impl DistributionEvidence {
    /// A fully unknown distribution (no source at all).
    pub fn missing() -> Self {
        Self {
            status: OsReleaseStatus::Missing,
            evidence: vec![Evidence::new(
                EvidenceSource::OsRelease,
                Confidence::Low,
                "os-release unavailable; distribution unknown",
            )],
            ..Self::default()
        }
    }

    /// The display name shown in the summary: `PRETTY_NAME`, falling back to
    /// `NAME`, then `ID`. Matching never uses this — IDs only.
    pub fn display_name(&self) -> Option<String> {
        self.pretty_name
            .clone()
            .or_else(|| self.name.clone())
            .or_else(|| self.id.clone())
    }

    /// ID-family match over `ID` and every `ID_LIKE` entry (case-insensitive
    /// — IDs are lowercase per spec, but tolerate upstream sloppiness).
    pub fn matches_family(&self, ids: &[&str]) -> bool {
        let want = |candidate: &str| ids.iter().any(|id| id.eq_ignore_ascii_case(candidate));
        self.id.as_deref().map(want).unwrap_or(false)
            || self.id_like.iter().any(|candidate| want(candidate))
    }
}

/// Informational kernel/architecture facts — compatibility diagnostics only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelInfo {
    pub release: String,
    pub arch: &'static str,
}

/// Where we appear to be running. Reported as evidence only; namespace and
/// container environments may legitimately report host identity that differs
/// from accessible services, so nothing here gates a capability by itself.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ExecutionContext {
    #[default]
    Host,
    Container {
        kind: String,
    },
    Wsl,
    Unknown,
}

/// Pure os-release parser: returns the parsed keys and the number of skipped
/// (malformed) lines. Future/unknown keys are kept — tolerance is the rule.
pub fn parse_os_release(text: &str) -> (BTreeMap<String, String>, usize) {
    let mut keys = BTreeMap::new();
    let mut skipped = 0usize;
    for line in text.lines() {
        let line = line.trim_start();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            skipped += 1;
            continue;
        };
        let key = key.trim();
        if key.is_empty()
            || !key
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        {
            skipped += 1;
            continue;
        }
        keys.insert(key.to_string(), unquote(value.trim()));
    }
    (keys, skipped)
}

/// Strip one layer of matching quotes per the spec (no escape processing —
/// the spec's `\` sequences inside double quotes are left as-is; we only
/// ever display these strings, never match on them).
fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
    {
        value[1..value.len() - 1].to_string()
    } else {
        value.to_string()
    }
}

/// Pure distribution-evidence builder from a parsed key map.
pub fn distribution_from_keys(
    keys: &BTreeMap<String, String>,
    skipped: usize,
) -> (DistributionEvidence, Vec<DetectionProblem>) {
    let mut problems = Vec::new();
    if skipped > 0 {
        problems.push(DetectionProblem {
            kind: ProblemKind::OsReleaseMalformed,
            message: "The distribution information file contains lines that could not be parsed."
                .to_string(),
            technical: format!("os-release: {skipped} malformed line(s) skipped"),
        });
    }
    let id = keys.get("ID").filter(|v| !v.is_empty()).cloned();
    let id_like = keys
        .get("ID_LIKE")
        .map(|v| v.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default();
    let evidence = vec![Evidence::new(
        EvidenceSource::OsRelease,
        if id.is_some() {
            Confidence::High
        } else {
            Confidence::Low
        },
        match &id {
            Some(id) => format!("ID={id}"),
            None => "no usable ID field".to_string(),
        },
    )];
    let status = if id.is_some() {
        OsReleaseStatus::Found
    } else {
        OsReleaseStatus::Malformed
    };
    (
        DistributionEvidence {
            id,
            id_like,
            name: keys.get("NAME").filter(|v| !v.is_empty()).cloned(),
            version_id: keys.get("VERSION_ID").filter(|v| !v.is_empty()).cloned(),
            pretty_name: keys.get("PRETTY_NAME").filter(|v| !v.is_empty()).cloned(),
            status,
            evidence,
        },
        problems,
    )
}

/// Pure execution-context classifier.
pub fn detect_execution_context(
    kernel_release: Option<&str>,
    dockerenv: bool,
    containerenv: bool,
    cgroup: Option<&str>,
) -> ExecutionContext {
    if let Some(release) = kernel_release
        && release.to_ascii_lowercase().contains("microsoft")
    {
        return ExecutionContext::Wsl;
    }
    if containerenv {
        return ExecutionContext::Container {
            kind: "podman".to_string(),
        };
    }
    if dockerenv {
        return ExecutionContext::Container {
            kind: "docker".to_string(),
        };
    }
    if let Some(cgroup) = cgroup {
        for marker in ["/docker/", "/containerd/", "/kubepods/", "/lxc/"] {
            if cgroup.contains(marker) {
                return ExecutionContext::Container {
                    kind: marker.trim_matches('/').to_string(),
                };
            }
        }
    }
    ExecutionContext::Host
}

/// Read the os-release file, kernel release and execution context. All
/// failures degrade to unknown/missing facts plus diagnostics — never errors.
pub fn collect(
    sources: &DiscoverySources,
) -> (
    DistributionEvidence,
    Option<KernelInfo>,
    ExecutionContext,
    u64,
    Vec<DetectionProblem>,
) {
    let started = Instant::now();

    let (distro, problems) = match &sources.os_release_path {
        Some(path) => match std::fs::read_to_string(path) {
            Ok(text) => {
                let (keys, skipped) = parse_os_release(&text);
                let (distro, problems) = distribution_from_keys(&keys, skipped);
                (distro, problems)
            }
            Err(_) => {
                log::info("core::discovery", "os-release unreadable", &[]);
                (DistributionEvidence::missing(), Vec::new())
            }
        },
        None => (DistributionEvidence::missing(), Vec::new()),
    };

    let kernel_release = sources
        .kernel_release_path
        .as_ref()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());
    let kernel = kernel_release.as_ref().map(|release| KernelInfo {
        release: release.clone(),
        arch: std::env::consts::ARCH,
    });

    let execution_context = detect_execution_context(
        kernel_release.as_deref(),
        std::path::Path::new("/.dockerenv").exists(),
        std::path::Path::new("/run/.containerenv").exists(),
        std::fs::read_to_string("/proc/1/cgroup").ok().as_deref(),
    );

    let micros = started.elapsed().as_micros() as u64;
    (distro, kernel, execution_context, micros, problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> BTreeMap<String, String> {
        parse_os_release(text).0
    }

    #[test]
    fn parses_standard_fields_and_quotes() {
        let keys = parse(
            "NAME=\"Arch Linux\"\nPRETTY_NAME=\"Arch Linux\"\nID=arch\n\
             ID_LIKE=arch\nVERSION_ID='20261001'\n# comment\n\nHOME_URL=\"https://archlinux.org/\"\n",
        );
        assert_eq!(keys["ID"], "arch");
        assert_eq!(keys["NAME"], "Arch Linux");
        assert_eq!(keys["VERSION_ID"], "20261001");
        assert_eq!(keys["HOME_URL"], "https://archlinux.org/");
    }

    #[test]
    fn tolerates_malformed_and_future_fields() {
        let (keys, skipped) = parse_os_release(
            "ID=fedora\nMALFORMED LINE WITHOUT EQUALS\n=FALLBACK_KEY\n2038_KEY=ok\nlow_key=nope\n",
        );
        assert_eq!(keys["ID"], "fedora");
        assert_eq!(keys["2038_KEY"], "ok"); // future fields kept
        assert!(!keys.contains_key("low_key")); // spec keys are uppercase
        assert_eq!(skipped, 3);
    }

    #[test]
    fn empty_and_missing_file_mean_unknown() {
        let (distro, problems) = distribution_from_keys(&BTreeMap::new(), 0);
        assert_eq!(distro.status, OsReleaseStatus::Malformed);
        assert_eq!(distro.display_name(), None);
        assert!(!distro.matches_family(&["arch"]));
        assert!(problems.is_empty());

        let empty = DistributionEvidence::missing();
        assert_eq!(empty.status, OsReleaseStatus::Missing);
        assert_eq!(empty.display_name(), None);
    }

    #[test]
    fn family_matching_uses_ids_not_pretty_names() {
        let distro = DistributionEvidence {
            id: Some("arch".into()),
            pretty_name: Some("Totally Not Debian".into()),
            ..DistributionEvidence::default()
        };
        assert!(distro.matches_family(&["arch", "alpine"]));
        assert!(distro.matches_family(&["ARCH"]));
        assert!(!distro.matches_family(&["debian"]));

        let distro = DistributionEvidence {
            id: Some("ubuntu".into()),
            id_like: vec!["debian".into()],
            ..distro
        };
        assert!(distro.matches_family(&["debian"]));
    }

    #[test]
    fn malformed_lines_become_a_problem_not_an_error() {
        let (keys, skipped) = parse_os_release("ID=debian\nbrokenline\n");
        assert_eq!(skipped, 1);
        let (distro, problems) = distribution_from_keys(&keys, skipped);
        assert_eq!(distro.status, OsReleaseStatus::Found);
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].kind, ProblemKind::OsReleaseMalformed);
    }

    #[test]
    fn execution_context_classifies_markers() {
        assert_eq!(
            detect_execution_context(Some("7.2.11-arch1-1"), false, false, None),
            ExecutionContext::Host
        );
        assert_eq!(
            detect_execution_context(
                Some("5.15.90.1-microsoft-standard-WSL2"),
                false,
                false,
                None
            ),
            ExecutionContext::Wsl
        );
        assert_eq!(
            detect_execution_context(None, true, false, None),
            ExecutionContext::Container {
                kind: "docker".into()
            }
        );
        assert_eq!(
            detect_execution_context(None, false, true, None),
            ExecutionContext::Container {
                kind: "podman".into()
            }
        );
        assert_eq!(
            detect_execution_context(None, false, false, Some("12:pids:/kubepods/pod123\n")),
            ExecutionContext::Container {
                kind: "kubepods".into()
            }
        );
    }
}
