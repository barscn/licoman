//! Redacted, non-secret diagnostics export (03_ENVIRONMENT_DISCOVERY.md
//! deliverables).
//!
//! The export contains discovery facts only: ids, counts, statuses and
//! evidence summaries. It never contains setting values, file contents,
//! `Exec` lines, icon payloads or credentials. Absolute paths under the
//! user's home are redacted to `~` via [`super::redact_path`].

use super::{BackendKind, BackendStatus, EnvironmentSnapshot};

/// Render the full diagnostics text. `home` is used for path redaction.
pub fn diagnostics_text(snapshot: &EnvironmentSnapshot, home: Option<&str>) -> String {
    // Every free-text fact passes through this: any absolute path under the
    // user's home becomes "~". Facts are ids/counts only otherwise.
    let redact = |text: &str| match home {
        Some(home) if !home.is_empty() => text.replace(home, "~"),
        _ => text.to_string(),
    };
    let mut out = String::new();
    out.push_str("Licoman environment diagnostics (read-only discovery facts)\n");
    out.push_str("=========================================================\n\n");

    // ---- Distribution ------------------------------------------------------
    out.push_str("distribution:\n");
    match &snapshot.distro.status {
        super::os_release::OsReleaseStatus::Found => {
            out.push_str(&format!(
                "  id={} id_like={}\n",
                snapshot.distro.id.as_deref().unwrap_or("?"),
                if snapshot.distro.id_like.is_empty() {
                    "-".to_string()
                } else {
                    snapshot.distro.id_like.join(",")
                }
            ));
            if let Some(pretty) = &snapshot.distro.pretty_name {
                out.push_str(&format!("  pretty_name={pretty}\n"));
            }
            if let Some(version) = &snapshot.distro.version_id {
                out.push_str(&format!("  version_id={version}\n"));
            }
        }
        super::os_release::OsReleaseStatus::Missing => {
            out.push_str("  unknown (os-release unavailable)\n");
        }
        super::os_release::OsReleaseStatus::Malformed => {
            out.push_str("  unknown (os-release unreadable or unusable)\n");
        }
    }
    if let Some(kernel) = &snapshot.kernel {
        out.push_str(&format!(
            "  kernel={} arch={}\n",
            kernel.release, kernel.arch
        ));
    }
    out.push_str(&format!(
        "  execution_context={}\n",
        match &snapshot.execution_context {
            super::os_release::ExecutionContext::Host => "host".to_string(),
            super::os_release::ExecutionContext::Container { kind } => {
                format!("container ({kind})")
            }
            super::os_release::ExecutionContext::Wsl => "wsl".to_string(),
            super::os_release::ExecutionContext::Unknown => "unknown".to_string(),
        }
    ));

    // ---- Session -----------------------------------------------------------
    out.push_str("\nsession:\n");
    out.push_str(&format!(
        "  desktop={} confidence={}\n",
        snapshot.session.desktop_display_name().unwrap_or("unknown"),
        snapshot
            .session
            .desktop_candidates
            .first()
            .map(|c| c.confidence.as_str())
            .unwrap_or("low")
    ));
    if let Some(compositor) = snapshot.session.compositor {
        out.push_str(&format!("  compositor={compositor}\n"));
    }
    out.push_str(&format!(
        "  protocol={}\n",
        snapshot
            .session
            .protocol_display_name()
            .unwrap_or("unknown")
    ));
    for evidence in snapshot
        .session
        .desktop_evidence
        .iter()
        .chain(&snapshot.session.protocol_evidence)
    {
        out.push_str(&format!(
            "  evidence source={} confidence={} detail={}\n",
            evidence.source.as_str(),
            evidence.confidence.as_str(),
            evidence.detail
        ));
    }
    if let Some(ambiguity) = &snapshot.session.ambiguity {
        out.push_str(&format!("  ambiguity: {ambiguity}\n"));
    }

    // ---- Package managers --------------------------------------------------
    let packages: Vec<&super::BackendCapability> = snapshot
        .available_backends
        .iter()
        .filter(|b| b.kind == BackendKind::PackageTool)
        .collect();
    if !packages.is_empty() {
        out.push_str("\npackage managers (informational, presence only):\n");
        for backend in packages {
            let name = backend
                .id
                .strip_prefix("package-manager:")
                .unwrap_or(&backend.id);
            out.push_str(&format!(
                "  {name}: {}\n",
                match &backend.status {
                    BackendStatus::Available => "found".to_string(),
                    BackendStatus::Unavailable { .. } => "not found".to_string(),
                }
            ));
        }
    }

    // ---- Applications ------------------------------------------------------
    out.push_str("\napplications:\n");
    let total = snapshot.installed_apps.len();
    let by_origin = |origin: &str| {
        snapshot
            .installed_apps
            .iter()
            .filter(|a| a.origin.as_str() == origin)
            .count()
    };
    out.push_str(&format!(
        "  discovered={total} (user={} system={} flatpak-user={} flatpak-system={})\n",
        by_origin("user"),
        by_origin("system"),
        by_origin("flatpak-user"),
        by_origin("flatpak-system"),
    ));
    let no_display = snapshot
        .installed_apps
        .iter()
        .filter(|a| a.no_display)
        .count();
    let restricted = snapshot
        .installed_apps
        .iter()
        .filter(|a| a.restricted_from_current)
        .count();
    out.push_str(&format!(
        "  no_display={no_display} restricted_from_current={restricted}\n"
    ));

    // ---- Backends ----------------------------------------------------------
    out.push_str("\nbackends:\n");
    for backend in &snapshot.available_backends {
        out.push_str(&format!(
            "  {}: {}\n",
            backend.id,
            match &backend.status {
                BackendStatus::Available => "available".to_string(),
                BackendStatus::Unavailable { reason } => format!("unavailable ({reason})"),
            }
        ));
        if let Some(detail) = &backend.detail {
            out.push_str(&format!("    {detail}\n"));
        }
    }

    // ---- Problems ----------------------------------------------------------
    if snapshot.problems.is_empty() {
        out.push_str("\nproblems: none\n");
    } else {
        out.push_str("\nproblems:\n");
        for problem in &snapshot.problems {
            out.push_str(&format!(
                "  {}: {}\n    technical: {}\n",
                problem.kind.as_str(),
                redact(&problem.message),
                redact(&problem.technical)
            ));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_contains_no_exec_or_icon_values() {
        let snapshot = crate::discovery::demo_fixtures::arch_hyprland_wayland();
        let text = diagnostics_text(&snapshot, Some("/home/demo"));
        assert!(!text.contains("exec="));
        assert!(!text.contains("alacritty --"));
        assert!(text.contains("desktop=Hyprland") || text.contains("Hyprland"));
    }

    #[test]
    fn export_redacts_home_paths() {
        let snapshot = crate::discovery::demo_fixtures::arch_hyprland_wayland();
        let text = diagnostics_text(&snapshot, Some("/home/demo"));
        assert!(!text.contains("/home/demo"));
    }
}
