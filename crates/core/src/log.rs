//! Structured, level-filtered logging to stderr.
//!
//! Privacy rules (02_FOUNDATION.md §5): records carry setting **ids**,
//! counts and error kinds — never setting values, file contents, credentials
//! or full configuration dumps. The level comes from `LICOMAN_LOG`
//! (`error` | `warn` | `info` | `debug`; default `warn`) so normal runs stay
//! quiet and diagnostics are opt-in.
//!
//! Timestamps are UTC RFC 3339 computed from the Unix clock without any
//! external dependency; the conversion is pure and unit tested.

use std::io::Write;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error = 0,
    Warn = 1,
    Info = 2,
    Debug = 3,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warn => "warn",
            Level::Info => "info",
            Level::Debug => "debug",
        }
    }

    fn parse(s: &str) -> Option<Level> {
        match s.trim().to_lowercase().as_str() {
            "error" => Some(Level::Error),
            "warn" | "warning" => Some(Level::Warn),
            "info" => Some(Level::Info),
            "debug" | "trace" => Some(Level::Debug),
            _ => None,
        }
    }
}

/// 0 = unset; otherwise `level as u8 + 1` so 0 can mean "not initialized".
static MAX_LEVEL: AtomicU8 = AtomicU8::new(0);

/// Initialize the level filter from `LICOMAN_LOG` (or `default`). Call once
/// at startup; later calls are no-ops so tests can't fight each other.
pub fn init_from_env(default: Level) {
    let requested = std::env::var("LICOMAN_LOG")
        .ok()
        .and_then(|v| Level::parse(&v))
        .unwrap_or(default);
    init(requested);
}

/// Set the level filter explicitly.
pub fn init(level: Level) {
    MAX_LEVEL.store(level as u8 + 1, Ordering::Relaxed);
}

fn max_level() -> Option<Level> {
    match MAX_LEVEL.load(Ordering::Relaxed) {
        0 => None,
        n => Some(match n - 1 {
            0 => Level::Error,
            1 => Level::Warn,
            2 => Level::Info,
            _ => Level::Debug,
        }),
    }
}

/// Whether a record at `level` would be emitted (exposed for tests).
pub fn enabled(level: Level) -> bool {
    max_level().is_some_and(|max| level <= max)
}

/// Emit one structured record to stderr if the level filter allows it.
pub fn log(level: Level, target: &str, message: &str, fields: &[(&str, String)]) {
    if !enabled(level) {
        return;
    }
    let ts = unix_now();
    let line = format_record(level, target, message, fields, ts);
    let mut err = std::io::stderr().lock();
    let _ = writeln!(err, "{line}");
}

pub fn error(target: &str, message: &str, fields: &[(&str, String)]) {
    log(Level::Error, target, message, fields);
}

pub fn warn(target: &str, message: &str, fields: &[(&str, String)]) {
    log(Level::Warn, target, message, fields);
}

pub fn info(target: &str, message: &str, fields: &[(&str, String)]) {
    log(Level::Info, target, message, fields);
}

pub fn debug(target: &str, message: &str, fields: &[(&str, String)]) {
    log(Level::Debug, target, message, fields);
}

/// Format one record. Pure and unit tested; `unix_ts` is seconds since the
/// Unix epoch. Field values are single-line-safe: whitespace and newlines
/// collapse so records stay parseable.
pub fn format_record(
    level: Level,
    target: &str,
    message: &str,
    fields: &[(&str, String)],
    unix_ts: u64,
) -> String {
    let mut line = format!(
        "ts={} level={} target={} msg={}",
        rfc3339(unix_ts),
        level.as_str(),
        target,
        collapse(message)
    );
    for (key, value) in fields {
        line.push(' ');
        line.push_str(key);
        line.push('=');
        line.push_str(&collapse(value));
    }
    line
}

fn collapse(text: &str) -> String {
    let mut collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        collapsed.push_str("\"\"");
    }
    collapsed
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Convert Unix seconds to an RFC 3339 UTC timestamp using the civil-from-days
/// algorithm (Howard Hinnant). Pure; no timezone or libc involvement.
pub fn rfc3339(unix_ts: u64) -> String {
    let days = unix_ts / 86_400;
    let secs_of_day = unix_ts % 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60
    )
}

/// Days since 1970-01-01 → (year, month, day) in the proleptic Gregorian
/// calendar.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_known_instants() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        // 2026-10-30T00:00:00Z (the date this phase was implemented).
        assert_eq!(rfc3339(1_793_318_400), "2026-10-30T00:00:00Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_709_164_799), "2024-02-28T23:59:59Z");
    }

    #[test]
    fn records_are_structured_and_single_line() {
        let line = format_record(
            Level::Info,
            "core::store",
            "apply complete",
            &[("applied", "2".into()), ("failed", "0".into())],
            1_793_318_400,
        );
        assert_eq!(
            line,
            "ts=2026-10-30T00:00:00Z level=info target=core::store msg=apply complete applied=2 failed=0"
        );
        let messy = format_record(
            Level::Warn,
            "t",
            "line one\nline two",
            &[("detail", "a  b".into())],
            0,
        );
        assert!(!messy.contains('\n'));
        assert!(messy.contains("msg=line one line two"));
        assert!(messy.contains("detail=a b"));
    }

    #[test]
    fn level_parsing_and_filtering() {
        assert_eq!(Level::parse("INFO"), Some(Level::Info));
        assert_eq!(Level::parse("warning"), Some(Level::Warn));
        assert_eq!(Level::parse("bogus"), None);
        init(Level::Warn);
        assert!(enabled(Level::Error));
        assert!(enabled(Level::Warn));
        assert!(!enabled(Level::Info));
        assert!(!enabled(Level::Debug));
    }
}
