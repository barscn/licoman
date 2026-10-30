//! Desktop/session facts (03_ENVIRONMENT_DISCOVERY.md §2).
//!
//! Environment variables are **hints, not security authority**. Multiple
//! indicators are combined (`XDG_CURRENT_DESKTOP` as a colon-delimited list,
//! `DESKTOP_SESSION`, `XDG_SESSION_DESKTOP`, `XDG_SESSION_TYPE`,
//! `WAYLAND_DISPLAY`, `DISPLAY`); every indicator becomes evidence with its
//! own confidence, and disagreements are reported as ambiguity instead of
//! being collapsed into one arbitrary winner.
//!
//! Desktop identity, compositor/window manager and display protocol are
//! three separate facts. The compositor name is a *knowledge table* over the
//! detected desktop (documented in docs/architecture/discovery.md), not a
//! probe — we never claim to have inspected the compositor itself.

use std::collections::BTreeMap;
use std::time::Instant;

use super::{Confidence, Evidence, EvidenceSource};

/// Known desktop identities. The list is deliberately limited to the
/// desktops the plan names; anything else is `Unknown` — we do not claim
/// detection is complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DesktopIdentity {
    #[default]
    Unknown,
    Gnome,
    Kde,
    Xfce,
    Cosmic,
    Hyprland,
    Sway,
}

impl DesktopIdentity {
    /// Map one raw indicator string (case-insensitive).
    pub fn from_indicator(raw: &str) -> Option<Self> {
        let name = raw.trim();
        if name.is_empty() {
            return None;
        }
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "gnome" | "ubuntu:gnome" => Some(Self::Gnome),
            "kde" | "plasma" | "plasma5" | "plasma6" | "kde-plasma" => Some(Self::Kde),
            "xfce" => Some(Self::Xfce),
            "cosmic" => Some(Self::Cosmic),
            "hyprland" => Some(Self::Hyprland),
            "sway" => Some(Self::Sway),
            _ => None,
        }
    }

    pub fn display_name(&self) -> Option<&'static str> {
        match self {
            Self::Gnome => Some("GNOME"),
            Self::Kde => Some("KDE Plasma"),
            Self::Xfce => Some("Xfce"),
            Self::Cosmic => Some("COSMIC"),
            Self::Hyprland => Some("Hyprland"),
            Self::Sway => Some("Sway"),
            Self::Unknown => None,
        }
    }

    /// Known desktop → compositor/WM pairing. A table, not a probe.
    pub fn known_compositor(&self) -> Option<&'static str> {
        match self {
            Self::Gnome => Some("Mutter"),
            Self::Kde => Some("KWin"),
            Self::Xfce => Some("Xfwm4"),
            Self::Cosmic => Some("cosmic-comp"),
            // Tiling Wayland compositors *are* their own window manager.
            Self::Hyprland => Some("Hyprland (compositor and WM are one process)"),
            Self::Sway => Some("Sway (compositor and WM are one process)"),
            Self::Unknown => None,
        }
    }
}

/// Display protocol — a fact separate from desktop identity ("GNOME on
/// Wayland" is not the same fact as "GNOME").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Protocol {
    Wayland,
    X11,
    Tty,
    #[default]
    Unknown,
}

impl Protocol {
    pub fn display_name(&self) -> Option<&'static str> {
        match self {
            Self::Wayland => Some("Wayland"),
            Self::X11 => Some("X11"),
            Self::Tty => Some("TTY"),
            Self::Unknown => None,
        }
    }
}

/// One indicator that voted for a desktop identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopCandidate {
    pub identity: DesktopIdentity,
    pub confidence: Confidence,
    /// Which variable supplied it (for evidence and diagnostics).
    pub source_var: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionEvidence {
    /// The best-supported desktop identity (first candidate of
    /// `XDG_CURRENT_DESKTOP` when present — the spec's preference order).
    pub desktop: DesktopIdentity,
    pub desktop_candidates: Vec<DesktopCandidate>,
    /// Known compositor/WM pairing for the detected desktop (table-based).
    pub compositor: Option<&'static str>,
    pub desktop_evidence: Vec<Evidence>,
    pub protocol: Protocol,
    pub protocol_evidence: Vec<Evidence>,
    /// Human-readable note when indicators disagree or the session looks
    /// nested/forwarded. Never a silent arbitrary winner.
    pub ambiguity: Option<String>,
}

impl SessionEvidence {
    pub fn desktop_display_name(&self) -> Option<&'static str> {
        self.desktop.display_name()
    }

    pub fn protocol_display_name(&self) -> Option<&'static str> {
        self.protocol.display_name()
    }
}

/// Pure protocol detection from the environment snapshot.
pub fn protocol_from_env(env: &BTreeMap<String, String>) -> (Protocol, Vec<Evidence>) {
    if let Some(value) = env.get("XDG_SESSION_TYPE").map(String::as_str) {
        let protocol = match value.to_ascii_lowercase().as_str() {
            "wayland" => Some(Protocol::Wayland),
            "x11" => Some(Protocol::X11),
            "tty" => Some(Protocol::Tty),
            _ => None,
        };
        if let Some(protocol) = protocol {
            return (
                protocol,
                vec![Evidence::new(
                    EvidenceSource::EnvVar("XDG_SESSION_TYPE"),
                    Confidence::High,
                    format!("XDG_SESSION_TYPE={value}"),
                )],
            );
        }
        // Unknown XDG_SESSION_TYPE value: keep it as evidence but do not
        // treat it as authoritative — fall through to socket hints.
        let mut evidence = vec![Evidence::new(
            EvidenceSource::EnvVar("XDG_SESSION_TYPE"),
            Confidence::Low,
            format!("XDG_SESSION_TYPE={value} (unrecognized)"),
        )];
        let (fallback, more) = protocol_from_sockets(env);
        evidence.extend(more);
        return (fallback, evidence);
    }
    protocol_from_sockets(env)
}

fn protocol_from_sockets(env: &BTreeMap<String, String>) -> (Protocol, Vec<Evidence>) {
    if env.contains_key("WAYLAND_DISPLAY") || env.contains_key("WAYLAND_SOCKET") {
        return (
            Protocol::Wayland,
            vec![Evidence::new(
                EvidenceSource::EnvVar("WAYLAND_DISPLAY"),
                Confidence::Medium,
                "Wayland socket variable present".to_string(),
            )],
        );
    }
    if env.contains_key("DISPLAY") {
        return (
            Protocol::X11,
            vec![Evidence::new(
                EvidenceSource::EnvVar("DISPLAY"),
                Confidence::Medium,
                "DISPLAY present".to_string(),
            )],
        );
    }
    (
        Protocol::Unknown,
        vec![Evidence::new(
            EvidenceSource::EnvVar("XDG_SESSION_TYPE"),
            Confidence::Low,
            "no session type or display socket variables".to_string(),
        )],
    )
}

/// Pure desktop-identity detection. Collects candidates from all three
/// indicator variables, resolves the best, and records ambiguity when the
/// variables disagree.
pub fn desktop_from_env(
    env: &BTreeMap<String, String>,
) -> (Vec<DesktopCandidate>, Vec<Evidence>, Option<String>) {
    let mut candidates = Vec::new();
    let mut evidence = Vec::new();

    if let Some(raw) = env.get("XDG_CURRENT_DESKTOP") {
        evidence.push(Evidence::new(
            EvidenceSource::EnvVar("XDG_CURRENT_DESKTOP"),
            Confidence::High,
            format!("XDG_CURRENT_DESKTOP={raw}"),
        ));
        // Colon-delimited list in preference order; each element is matched
        // separately — never a false single-string match.
        for element in raw.split(':').filter(|e| !e.trim().is_empty()) {
            if let Some(identity) = DesktopIdentity::from_indicator(element) {
                candidates.push(DesktopCandidate {
                    identity,
                    confidence: Confidence::High,
                    source_var: "XDG_CURRENT_DESKTOP",
                });
            }
        }
    }
    if let Some(raw) = env.get("XDG_SESSION_DESKTOP") {
        evidence.push(Evidence::new(
            EvidenceSource::EnvVar("XDG_SESSION_DESKTOP"),
            Confidence::Medium,
            format!("XDG_SESSION_DESKTOP={raw}"),
        ));
        if let Some(identity) = DesktopIdentity::from_indicator(raw) {
            candidates.push(DesktopCandidate {
                identity,
                confidence: Confidence::Medium,
                source_var: "XDG_SESSION_DESKTOP",
            });
        }
    }
    if let Some(raw) = env.get("DESKTOP_SESSION") {
        evidence.push(Evidence::new(
            EvidenceSource::EnvVar("DESKTOP_SESSION"),
            Confidence::Low,
            format!("DESKTOP_SESSION={raw}"),
        ));
        // Often a full path or a generic name like "default" — use the file
        // stem and treat it as the weakest hint.
        let stem = raw.rsplit('/').next().unwrap_or(raw);
        if let Some(identity) = DesktopIdentity::from_indicator(stem) {
            candidates.push(DesktopCandidate {
                identity,
                confidence: Confidence::Low,
                source_var: "DESKTOP_SESSION",
            });
        }
    }

    // Ambiguity: two variables with High/Medium confidence disagree.
    let mut ambiguity = None;
    let distinct: Vec<DesktopIdentity> = {
        let mut seen = Vec::new();
        for candidate in &candidates {
            if !seen.contains(&candidate.identity) {
                seen.push(candidate.identity);
            }
        }
        seen
    };
    if distinct.len() > 1 {
        let names: Vec<String> = distinct
            .iter()
            .filter_map(|identity| identity.display_name())
            .map(str::to_string)
            .collect();
        ambiguity = Some(format!(
            "Session indicators disagree: {}. Highest-confidence candidate is used.",
            names.join(" vs ")
        ));
    }
    (candidates, evidence, ambiguity)
}

/// Detect the session facts from an environment snapshot. Pure (env is
/// injected), cheap, and side-effect free.
pub fn detect(env: &BTreeMap<String, String>) -> (SessionEvidence, u64) {
    let started = Instant::now();
    let (candidates, desktop_evidence, ambiguity) = desktop_from_env(env);

    let desktop = candidates
        .first()
        .map(|candidate| candidate.identity)
        .unwrap_or(DesktopIdentity::Unknown);
    let compositor = desktop.known_compositor();

    let (protocol, protocol_evidence) = protocol_from_env(env);

    // Nested/forwarded session note: both Wayland and X11 sockets visible.
    let mut notes_evidence = Vec::new();
    let wayland_visible = env.contains_key("WAYLAND_DISPLAY") || env.contains_key("WAYLAND_SOCKET");
    let x11_visible = env.contains_key("DISPLAY");
    let ambiguity = if wayland_visible && x11_visible {
        let note = "Both Wayland and X11 sockets are visible in this session; the session may be \
                    nested or bridged (XWayland). Discovery treats them as display hints only."
            .to_string();
        notes_evidence.push(Evidence::new(
            EvidenceSource::EnvVar("WAYLAND_DISPLAY"),
            Confidence::Low,
            "both WAYLAND_DISPLAY and DISPLAY present".to_string(),
        ));
        Some(match ambiguity {
            Some(existing) => format!("{existing} {note}"),
            None => note,
        })
    } else {
        ambiguity
    };

    let session = SessionEvidence {
        desktop,
        desktop_candidates: candidates,
        compositor,
        desktop_evidence,
        protocol,
        protocol_evidence: protocol_evidence
            .into_iter()
            .chain(notes_evidence)
            .collect(),
        ambiguity,
    };
    (session, started.elapsed().as_micros() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    // ---- Test-matrix rows -------------------------------------------------

    #[test]
    fn arch_hyprland_wayland_identified() {
        let env = env(&[
            ("XDG_CURRENT_DESKTOP", "Hyprland"),
            ("XDG_SESSION_DESKTOP", "Hyprland"),
            ("XDG_SESSION_TYPE", "wayland"),
            ("WAYLAND_DISPLAY", "wayland-1"),
        ]);
        let (session, _) = detect(&env);
        assert_eq!(session.desktop, DesktopIdentity::Hyprland);
        assert_eq!(session.protocol, Protocol::Wayland);
        assert!(session.ambiguity.is_none());
        assert!(
            session
                .compositor
                .is_some_and(|c| c.contains("one process"))
        );
    }

    #[test]
    fn fedora_gnome_wayland_detected_with_separate_protocol_fact() {
        let env = env(&[
            ("XDG_CURRENT_DESKTOP", "GNOME"),
            ("XDG_SESSION_DESKTOP", "gnome"),
            ("XDG_SESSION_TYPE", "wayland"),
            ("WAYLAND_DISPLAY", "wayland-0"),
        ]);
        let (session, _) = detect(&env);
        assert_eq!(session.desktop, DesktopIdentity::Gnome);
        assert_eq!(session.protocol, Protocol::Wayland);
        assert_eq!(session.compositor, Some("Mutter"));
    }

    #[test]
    fn ubuntu_gnome_x11_correctly_identified() {
        let env = env(&[
            ("XDG_CURRENT_DESKTOP", "ubuntu:GNOME"),
            ("XDG_SESSION_DESKTOP", "ubuntu"),
            ("XDG_SESSION_TYPE", "x11"),
            ("DISPLAY", ":0"),
        ]);
        let (session, _) = detect(&env);
        assert_eq!(session.desktop, DesktopIdentity::Gnome);
        assert_eq!(session.protocol, Protocol::X11);
    }

    #[test]
    fn debian_xfce_desktop_distro_and_pm_not_conflated() {
        // The session module must only look at session variables.
        let env = env(&[
            ("XDG_CURRENT_DESKTOP", "XFCE"),
            ("XDG_SESSION_DESKTOP", "xfce"),
            ("XDG_SESSION_TYPE", "x11"),
            ("DISPLAY", ":0"),
        ]);
        let (session, _) = detect(&env);
        assert_eq!(session.desktop, DesktopIdentity::Xfce);
        assert_eq!(session.protocol, Protocol::X11);
        // Nothing in this module ever mentions packages.
        let rendered = format!("{session:?}");
        assert!(!rendered.contains("apt"));
        assert!(!rendered.contains("package"));
    }

    #[test]
    fn multi_value_xdg_current_desktop_has_no_false_single_string_match() {
        let first = env(&[
            ("XDG_CURRENT_DESKTOP", "ubuntu:GNOME"),
            ("XDG_SESSION_TYPE", "x11"),
        ]);
        let (candidates, _, _) = desktop_from_env(&first);
        // "ubuntu:GNOME" must yield GNOME via colon-splitting, not Unknown.
        assert_eq!(
            candidates.first().map(|c| c.identity),
            Some(DesktopIdentity::Gnome)
        );

        // An unknown first element followed by a known one still detects.
        let second = env(&[
            ("XDG_CURRENT_DESKTOP", "PicoFire:GNOME"),
            ("XDG_SESSION_TYPE", "wayland"),
        ]);
        let (candidates, _, _) = desktop_from_env(&second);
        assert_eq!(
            candidates.first().map(|c| c.identity),
            Some(DesktopIdentity::Gnome)
        );
    }

    #[test]
    fn disagreement_is_reported_not_resolved_silently() {
        let env = env(&[
            ("XDG_CURRENT_DESKTOP", "KDE"),
            ("XDG_SESSION_DESKTOP", "gnome"),
            ("XDG_SESSION_TYPE", "wayland"),
        ]);
        let (session, _) = detect(&env);
        assert_eq!(session.desktop, DesktopIdentity::Kde); // highest confidence wins
        let note = session.ambiguity.expect("ambiguity recorded");
        assert!(note.contains("disagree"), "note: {note}");
        // Both candidates remain inspectable.
        assert_eq!(session.desktop_candidates.len(), 2);
    }

    #[test]
    fn missing_indicators_mean_unknown_and_app_stays_usable() {
        let (session, _) = detect(&BTreeMap::new());
        assert_eq!(session.desktop, DesktopIdentity::Unknown);
        assert_eq!(session.protocol, Protocol::Unknown);
        assert_eq!(session.desktop_display_name(), None);
        assert_eq!(session.protocol_display_name(), None);
        assert_eq!(session.compositor, None);
    }

    #[test]
    fn desktop_session_generic_name_is_ignored() {
        let env = env(&[
            ("DESKTOP_SESSION", "default"),
            ("XDG_SESSION_TYPE", "x11"),
            ("DISPLAY", ":0"),
        ]);
        let (session, _) = detect(&env);
        assert_eq!(session.desktop, DesktopIdentity::Unknown);
        assert_eq!(session.protocol, Protocol::X11);
    }

    #[test]
    fn tty_session_recognized() {
        let env = env(&[("XDG_SESSION_TYPE", "tty")]);
        let (session, _) = detect(&env);
        assert_eq!(session.protocol, Protocol::Tty);
    }

    #[test]
    fn nested_wayland_x11_sessions_note_instead_of_crash() {
        let env = env(&[
            ("XDG_CURRENT_DESKTOP", "Hyprland"),
            ("XDG_SESSION_TYPE", "wayland"),
            ("WAYLAND_DISPLAY", "wayland-1"),
            ("DISPLAY", ":1"),
        ]);
        let (session, _) = detect(&env);
        let note = session.ambiguity.expect("nested note recorded");
        assert!(note.contains("nested"), "note: {note}");
        assert_eq!(session.protocol, Protocol::Wayland);
    }

    #[test]
    fn unknown_session_type_falls_back_to_sockets() {
        let vars = env(&[
            ("XDG_SESSION_TYPE", "futuristic-protocol"),
            ("WAYLAND_DISPLAY", "wayland-9"),
        ]);
        let (session, _) = detect(&vars);
        assert_eq!(session.protocol, Protocol::Wayland);
        assert!(
            session
                .protocol_evidence
                .iter()
                .any(|e| e.detail.contains("unrecognized"))
        );
    }
}
