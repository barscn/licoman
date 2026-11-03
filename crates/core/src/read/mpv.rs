//! mpv version detection and configuration grammar (Phase 08, Wave B): the
//! reference application for the second file-backend resolution.
//!
//! Facts encoded here come from upstream and were live-verified against the
//! installed mpv 0.41.0 (`mpv --version`, `mpv --list-options`):
//!
//! - The version query prints `mpv v0.41.0 Copyright …` — note the **leading
//!   `v`** on the version token.
//! - The user configuration is a **single file**: `$XDG_CONFIG_HOME/mpv/
//!   mpv.conf` (the legacy `~/.mpv/config` is also read when it is the only
//!   one present). There is no import system — first existing candidate
//!   wins, and a diagnostic lists what was checked.
//! - File grammar (mpv(1) manual, "Paragraph titled Configuration Files"):
//!   `option=value` lines, `#` comment lines, and `[profile-name]` section
//!   headers. Values run to end of line; there is no quoting layer. Keys
//!   below a section header belong to that profile — this build reads and
//!   edits **top-level** options only, and says so.
//! - The options this build exposes (`fullscreen`, `keep-open`,
//!   `save-position-on-quit`, `volume`, `osc`) keep their names, value
//!   shapes and defaults across mpv 0.33 – 0.41 per the upstream manual; the
//!   write gate requires a detected version ≥ [`CONFIG_ERA_MIN`], and an
//!   undetectable version disables writes while reads stay available
//!   (fail-closed, mirroring Alacritty's D50 gate).

use std::path::PathBuf;
use std::time::Duration;

/// How long `mpv --version` may take before it is killed and the version is
/// treated as undetectable.
pub const VERSION_TIMEOUT: Duration = Duration::from_secs(3);

/// Minimum detected version for **writes**: the exposed options are
/// documented with the same names, value shapes and defaults across
/// 0.33 – 0.41 (upstream manual; live-verified against 0.41.0). Reads are
/// not gated.
pub const CONFIG_ERA_MIN: (u32, u32, u32) = (0, 33, 0);

/// A parsed mpv version (`mpv v0.41.0` → 0.41.0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    /// The raw version token as reported (for display).
    pub raw: String,
}

impl AppVersion {
    pub fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
            raw: format!("{major}.{minor}.{patch}"),
        }
    }

    pub fn at_least(&self, min: (u32, u32, u32)) -> bool {
        (self.major, self.minor, self.patch) >= min
    }
}

/// Parse the exact output of `mpv --version` (documented form:
/// `mpv v0.41.0 Copyright …`). Strict: anything else is undetectable.
pub fn parse_version_output(output: &str) -> Option<AppVersion> {
    let line = output.lines().next()?.trim();
    let mut parts = line.split_whitespace();
    let program = parts.next()?;
    if !program.eq_ignore_ascii_case("mpv") {
        return None;
    }
    let version = parts.next()?;
    parse_version(version).map(|mut parsed| {
        parsed.raw = version.trim_start_matches('v').to_string();
        parsed
    })
}

/// Parse a bare `X.Y.Z` (or `X.Y`) version token, tolerating the `v` prefix
/// and pre-release suffixes.
pub fn parse_version(token: &str) -> Option<AppVersion> {
    let token = token.trim().trim_start_matches('v');
    let token = token.split('-').next()?; // tolerate pre-release suffixes
    let mut numbers = [0u32; 3];
    for (index, part) in token.split('.').enumerate() {
        if index > 2 {
            return None;
        }
        numbers[index] = part.parse().ok()?;
    }
    let raw = token.to_string();
    Some(AppVersion {
        major: numbers[0],
        minor: numbers[1],
        patch: numbers[2],
        raw,
    })
}

/// The documented user-configuration candidates in preference order
/// (mpv(1) FILES). `system_roots` contributes `<root>/mpv/mpv.conf`
/// entries (the system-wide locations).
pub fn config_candidates(
    config_home: &std::path::Path,
    home: &std::path::Path,
    system_roots: &[std::path::PathBuf],
) -> Vec<PathBuf> {
    let mut candidates = vec![
        config_home.join("mpv/mpv.conf"),
        home.join(".config/mpv/mpv.conf"),
        home.join(".mpv/config"),
    ];
    for root in system_roots {
        candidates.push(root.join("mpv/mpv.conf"));
    }
    candidates.dedup();
    candidates
}

/// One parsed line of an mpv configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MpvLine {
    Comment(String),
    Blank,
    /// `[name]` — everything after this header belongs to the named profile.
    SectionHeader(String),
    /// `key=value` with the raw (trimmed) value text.
    Entry {
        key: String,
        value: String,
    },
    /// Anything else (a key without `=`, malformed) — preserved verbatim,
    /// never edited.
    Other(String),
}

/// Parse an mpv configuration into classified lines (diagnostics and
/// probes). The writer works on the raw lines directly so everything is
/// preserved byte-for-byte except the one line it rewrites.
pub fn parse_conf(text: &str) -> Vec<MpvLine> {
    text.lines()
        .map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                MpvLine::Blank
            } else if trimmed.starts_with('#') {
                MpvLine::Comment(trimmed.to_string())
            } else if trimmed.starts_with('[') && trimmed.ends_with(']') && trimmed.len() >= 2 {
                MpvLine::SectionHeader(trimmed[1..trimmed.len() - 1].to_string())
            } else if let Some((key, value)) = trimmed.split_once('=') {
                let key = key.trim().to_string();
                let value = value.trim().to_string();
                if key.is_empty() {
                    MpvLine::Other(trimmed.to_string())
                } else {
                    MpvLine::Entry { key, value }
                }
            } else {
                MpvLine::Other(trimmed.to_string())
            }
        })
        .collect()
}

/// Classify one raw line (the writer's per-line view).
pub fn classify_raw_line(line: &str) -> MpvLine {
    parse_conf(line)
        .into_iter()
        .next()
        .unwrap_or(MpvLine::Other(line.to_string()))
}

/// The raw-line index of the **top-level** entry for `key` — an entry that
/// appears before the first section header. `None` when the key is absent
/// from the top level (a profile-scoped same-name key does **not** count).
/// The returned index is into `raw_lines` (the writer splices that line).
pub fn find_top_level_index(raw_lines: &[&str], key: &str) -> Option<usize> {
    let mut in_section = false;
    for (index, line) in raw_lines.iter().enumerate() {
        match classify_raw_line(line) {
            MpvLine::SectionHeader(_) => in_section = true,
            MpvLine::Entry { key: entry_key, .. } if !in_section && entry_key == key => {
                return Some(index);
            }
            _ => {}
        }
    }
    None
}

/// The raw-line index **before which** a new top-level entry must be
/// inserted: after the last top-level entry (or comment/blank run) and
/// before the first section header. `None` means "append at the end"
/// (the file has no profile sections).
pub fn top_level_insert_index(raw_lines: &[&str]) -> Option<usize> {
    raw_lines
        .iter()
        .position(|line| matches!(classify_raw_line(line), MpvLine::SectionHeader(_)))
}

/// Render a [`SettingValue`](crate::value::SettingValue) as mpv.conf value
/// text. Bools are `yes`/`no` (the mpv flag spelling); everything else is
/// the value's own display form. Lists have no mpv.conf scalar spelling and
/// are rejected.
pub fn value_to_conf_text(value: &crate::value::SettingValue) -> Option<String> {
    use crate::value::SettingValue;
    Some(match value {
        SettingValue::Bool(true) => "yes".to_string(),
        SettingValue::Bool(false) => "no".to_string(),
        SettingValue::Integer(n) => n.to_string(),
        SettingValue::Float(f) => {
            // mpv parses floats; keep a plain representation.
            let text = format!("{f}");
            text
        }
        SettingValue::Text(text) | SettingValue::Choice(text) | SettingValue::Path(text) => {
            text.clone()
        }
        SettingValue::List(_) => return None,
    })
}

/// Interpret a raw mpv.conf value as the [`SettingValue`] the descriptor
/// declares. `yes/no/true/false/on/off` are bool spellings; numbers parse
/// per the type; anything else is an error, never a guess.
pub fn conf_text_to_value(
    raw: &str,
    setting_type: crate::value::SettingType,
) -> Option<crate::value::SettingValue> {
    use crate::value::{SettingType, SettingValue};
    match setting_type {
        SettingType::Bool => match raw.trim().to_ascii_lowercase().as_str() {
            "yes" | "on" | "true" => Some(SettingValue::Bool(true)),
            "no" | "off" | "false" => Some(SettingValue::Bool(false)),
            _ => None,
        },
        SettingType::Integer => raw.trim().parse::<i64>().ok().map(SettingValue::Integer),
        SettingType::Float => raw.trim().parse::<f64>().ok().map(SettingValue::Float),
        SettingType::Choice => Some(SettingValue::Choice(raw.trim().to_string())),
        SettingType::Path => Some(SettingValue::Path(raw.trim().to_string())),
        SettingType::Text => Some(SettingValue::Text(raw.trim().to_string())),
        SettingType::List => None,
    }
}

/// What version detection concluded for this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MpvSupport {
    /// A detected version that satisfies [`CONFIG_ERA_MIN`]; edits allowed.
    Supported(AppVersion),
    /// Detected version predates the documented configuration era.
    Legacy(AppVersion),
    /// No binary on `PATH`, the query timed out, or the output did not
    /// parse. Writes are refused (fail-closed).
    Unknown,
}

impl MpvSupport {
    /// Human-readable support verdict for status lines and Source details.
    pub fn describe(&self) -> String {
        match self {
            MpvSupport::Supported(version) => {
                format!("mpv {} (documented configuration, ≥ 0.33)", version.raw)
            }
            MpvSupport::Legacy(version) => format!(
                "mpv {} predates the configuration era this build documents \
                 (≥ 0.33); editing stays disabled",
                version.raw
            ),
            MpvSupport::Unknown => {
                "mpv version could not be verified — editing stays disabled until \
                 the version is known"
                    .to_string()
            }
        }
    }

    /// Whether writes to the configuration may be considered at all.
    pub fn writable(&self) -> bool {
        matches!(self, MpvSupport::Supported(_))
    }
}

/// Run `mpv --version` with a hard bound and parse the result. Direct exec
/// of the documented version query — never a shell, never anything else.
/// Any failure is [`MpvSupport::Unknown`].
pub fn detect() -> MpvSupport {
    match super::alacritty::detect_binary("mpv", VERSION_TIMEOUT) {
        Some(output) => match parse_version_output(&output) {
            Some(version) if version.at_least(CONFIG_ERA_MIN) => MpvSupport::Supported(version),
            Some(version) => MpvSupport::Legacy(version),
            None => MpvSupport::Unknown,
        },
        None => MpvSupport::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::{SettingType, SettingValue};

    #[test]
    fn version_output_parses_with_the_v_prefix() {
        let parsed = parse_version_output("mpv v0.41.0 Copyright © 2000-2025\n").expect("parses");
        assert_eq!((parsed.major, parsed.minor, parsed.patch), (0, 41, 0));
        assert_eq!(parsed.raw, "0.41.0");
        // Older builds printed without the leading `v`.
        let parsed = parse_version_output("mpv 0.34.1\n").expect("parses");
        assert_eq!((parsed.major, parsed.minor, parsed.patch), (0, 34, 1));
        assert!(parse_version_output("some other tool 1.0").is_none());
        assert!(parse_version_output("").is_none());
    }

    #[test]
    fn era_gate() {
        assert!(AppVersion::new(0, 33, 0).at_least(CONFIG_ERA_MIN));
        assert!(!AppVersion::new(0, 32, 9).at_least(CONFIG_ERA_MIN));
        assert!(MpvSupport::Supported(AppVersion::new(0, 41, 0)).writable());
        assert!(!MpvSupport::Legacy(AppVersion::new(0, 32, 9)).writable());
        assert!(!MpvSupport::Unknown.writable());
    }

    #[test]
    fn conf_lines_parse_losslessly() {
        let lines =
            parse_conf("# comment\n\nfullscreen=yes\n[myprofile]\nvolume=80\nkeep-open=no\n");
        assert_eq!(lines[0], MpvLine::Comment("# comment".into()));
        assert_eq!(lines[1], MpvLine::Blank);
        assert_eq!(
            lines[2],
            MpvLine::Entry {
                key: "fullscreen".into(),
                value: "yes".into()
            }
        );
        assert_eq!(lines[3], MpvLine::SectionHeader("myprofile".into()));
        assert_eq!(
            lines[4],
            MpvLine::Entry {
                key: "volume".into(),
                value: "80".into()
            }
        );

        let raw: Vec<&str> = "# comment\n\nfullscreen=yes\n[myprofile]\nvolume=80\n"
            .lines()
            .collect();
        // Top-level lookups by raw-line index; profile-scoped entries and
        // same-name traps are invisible.
        assert_eq!(find_top_level_index(&raw, "fullscreen"), Some(2));
        assert_eq!(find_top_level_index(&raw, "volume"), None);
        assert_eq!(find_top_level_index(&raw, "keep-open"), None);
        // Insertion goes before the first section header.
        assert_eq!(top_level_insert_index(&raw), Some(3));
        let no_sections: Vec<&str> = "fullscreen=yes\n".lines().collect();
        assert_eq!(top_level_insert_index(&no_sections), None);
        // Leading whitespace and indented keys still classify.
        assert_eq!(
            classify_raw_line("  fullscreen = yes  "),
            MpvLine::Entry {
                key: "fullscreen".into(),
                value: "yes".into()
            }
        );
    }

    #[test]
    fn value_round_trips_through_conf_text() {
        assert_eq!(
            value_to_conf_text(&SettingValue::Bool(true)).unwrap(),
            "yes"
        );
        assert_eq!(
            value_to_conf_text(&SettingValue::Bool(false)).unwrap(),
            "no"
        );
        assert_eq!(
            value_to_conf_text(&SettingValue::Choice("always".into())).unwrap(),
            "always"
        );
        assert!(value_to_conf_text(&SettingValue::List(vec![])).is_none());

        assert_eq!(
            conf_text_to_value("yes", SettingType::Bool),
            Some(SettingValue::Bool(true))
        );
        assert_eq!(
            conf_text_to_value("no", SettingType::Bool),
            Some(SettingValue::Bool(false))
        );
        assert_eq!(
            conf_text_to_value("80", SettingType::Integer),
            Some(SettingValue::Integer(80))
        );
        assert!(conf_text_to_value("maybe", SettingType::Bool).is_none());
        assert!(conf_text_to_value("a b", SettingType::Integer).is_none());
    }
}
