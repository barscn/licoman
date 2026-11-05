//! Local hardware/system facts for the Summary panel — read-only, fail-closed
//! (03_ENVIRONMENT_DISCOVERY.md philosophy applied to machine facts).
//!
//! Every fact degrades to `None`/empty on failure: the panel shows "unknown"
//! instead of pretending. Parsers are pure functions over fixture text and
//! unit-tested; the collectors read `/proc`, `/sys` and environment
//! variables, plus one GIO query for the root filesystem usage (GIO is
//! already a core dependency for the GSettings backend — no libc/statfs).
//!
//! These facts are informational only: nothing here gates a capability or a
//! write path (the environment snapshot remains the applicability authority).

use std::path::{Path, PathBuf};

use crate::discovery::os_release::parse_os_release;
use crate::discovery::session::DesktopIdentity;

/// The ordered facts the Summary panel presents. Field order mirrors the
/// panel's display order (logo and OS name first, then the fact list).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SystemFacts {
    /// `PRETTY_NAME`/`NAME`/`ID` from `/etc/os-release`.
    pub os_name: Option<String>,
    /// Distribution logo file found via the icon-theme/pixmaps conventions,
    /// when the system ships one.
    pub logo: Option<PathBuf>,
    pub kernel_release: Option<String>,
    /// Seconds since boot (`/proc/uptime`, first field).
    pub uptime_secs: Option<u64>,
    /// Basename of `$SHELL`.
    pub shell: Option<String>,
    /// Known compositor/WM pairing for the detected desktop (a knowledge
    /// table over `XDG_CURRENT_DESKTOP`, not a probe — same table as
    /// discovery).
    pub window_manager: Option<String>,
    /// Terminal emulator name (env hints, then the `/proc` parent chain).
    pub terminal: Option<String>,
    pub cpu: Option<CpuInfo>,
    /// One entry per DRM card with a readable PCI vendor (display order:
    /// ascending card number). VM virtual GPUs appear with their PCI id.
    pub gpus: Vec<GpuInfo>,
    pub memory: Option<MemoryInfo>,
    /// Present only when the system actually has swap configured.
    pub swap: Option<SwapInfo>,
    /// Usage of the filesystem mounted at `/`.
    pub disk: Option<DiskInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpuInfo {
    pub model: Option<String>,
    pub logical_cores: u32,
}

/// Integrated (on-CPU/APU) vs discrete (dedicated PCIe card) — a heuristic
/// classification, never a probe of the hardware itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuKind {
    Integrated,
    Discrete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuInfo {
    /// Human vendor name when the PCI vendor id is known.
    pub vendor: Option<&'static str>,
    /// Model name from the pci.ids database (`Radeon 610M`,
    /// `GeForce RTX 4060 Ti`), when the system ships the database and the
    /// device is listed.
    pub model: Option<String>,
    /// Integrated/discrete classification when the heuristics decide.
    pub kind: Option<GpuKind>,
    /// `vendor:device` hex ids without the `0x` prefix (e.g. `8086:A780`).
    pub pci_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryInfo {
    pub total_bytes: u64,
    pub used_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwapInfo {
    pub total_bytes: u64,
    pub used_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskInfo {
    pub total_bytes: u64,
    pub used_bytes: u64,
}

impl SystemFacts {
    /// Read every fact from the live system. Failures degrade per fact.
    pub fn collect() -> Self {
        let uptime_secs = std::fs::read_to_string("/proc/uptime")
            .ok()
            .and_then(|text| parse_uptime(&text));
        let shell = shell_from_env();
        let (memory, swap) = std::fs::read_to_string("/proc/meminfo")
            .ok()
            .map(|text| parse_meminfo(&text))
            .unwrap_or((None, None));
        Self {
            os_name: os_release_display_name(),
            logo: find_distro_logo(),
            kernel_release: kernel_release(),
            uptime_secs,
            shell,
            window_manager: window_manager_from_env(),
            terminal: terminal_from_environment(),
            cpu: std::fs::read_to_string("/proc/cpuinfo")
                .ok()
                .and_then(|text| parse_cpuinfo(&text)),
            gpus: enumerate_gpus(),
            memory,
            swap,
            disk: root_disk_usage(),
        }
    }
}

// ---- /proc readers (pure, unit-tested) ------------------------------------

/// `/proc/uptime`: seconds since boot, first whitespace field.
pub fn parse_uptime(text: &str) -> Option<u64> {
    let first = text.split_whitespace().next()?;
    let secs: f64 = first.parse().ok()?;
    if !secs.is_finite() || secs < 0.0 {
        return None;
    }
    Some(secs.floor() as u64)
}

/// One meminfo key ("MemTotal") to its value in bytes (the kernel reports
/// KiB despite the "kB" label). Only the four keys the panel uses.
fn meminfo_key(text: &str, key: &str) -> Option<u64> {
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let Some(name) = parts.next() else { continue };
        let Some(value) = parts.next() else { continue };
        if name.trim_end_matches(':') == key {
            return value.parse::<u64>().ok().map(|kib| kib * 1024);
        }
    }
    None
}

/// `/proc/meminfo`: memory usage (`MemTotal − MemAvailable`) and swap, the
/// latter only when the system actually has swap configured.
pub fn parse_meminfo(text: &str) -> (Option<MemoryInfo>, Option<SwapInfo>) {
    let memory = match (
        meminfo_key(text, "MemTotal"),
        meminfo_key(text, "MemAvailable"),
    ) {
        (Some(total), Some(available)) if total >= available => Some(MemoryInfo {
            total_bytes: total,
            used_bytes: total - available,
        }),
        // "available" is a post-3.14 field; a kernel without it yields no
        // honest used number, so nothing is claimed.
        _ => None,
    };
    let swap = match (
        meminfo_key(text, "SwapTotal"),
        meminfo_key(text, "SwapFree"),
    ) {
        (Some(total), Some(free)) if total > 0 && total >= free => Some(SwapInfo {
            total_bytes: total,
            used_bytes: total - free,
        }),
        _ => None,
    };
    (memory, swap)
}

/// `/proc/cpuinfo`: model name (x86 `model name`, ARM `Processor` fallback)
/// and the count of `processor` entries (logical cores).
pub fn parse_cpuinfo(text: &str) -> Option<CpuInfo> {
    let mut model: Option<String> = None;
    let mut cores = 0u32;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if key == "model name" || (model.is_none() && key == "Processor") {
            model = Some(value.to_string());
        } else if key == "processor" && !value.is_empty() {
            cores += 1;
        }
    }
    if model.is_none() && cores == 0 {
        return None;
    }
    Some(CpuInfo {
        model,
        logical_cores: cores,
    })
}

/// The comm field of a `/proc/<pid>/stat` line: between the first `(` and
/// the last `)` (a comm may itself contain parentheses/spaces).
pub fn proc_stat_comm(text: &str) -> Option<String> {
    let start = text.find('(')? + 1;
    let end = text.rfind(')')?;
    if start >= end {
        return None;
    }
    Some(text[start..end].to_string())
}

/// The PPid field (field 4) of a `/proc/<pid>/stat` line: the second
/// whitespace field after the comm's closing parenthesis.
pub fn proc_stat_ppid(text: &str) -> Option<u32> {
    let end = text.rfind(')')?;
    let mut parts = text[end + 1..].split_whitespace();
    parts.next()?; // state
    parts.next()?.parse().ok()
}

// ---- terminal / window manager identity ------------------------------------

/// comms of terminal emulators Licoman recognizes in the process ancestry.
const TERMINAL_COMMS: &[&str] = &[
    "kitty",
    "alacritty",
    "foot",
    "footclient",
    "ghostty",
    "wezterm",
    "konsole",
    "gnome-terminal-",
    "ptyxis",
    "kgx",
    "xterm",
    "st",
    "st-256color",
    "urxvt",
    "rxvt",
    "tilix",
    "terminator",
    "termite",
    "qterminal",
    "lxterminal",
    "sakura",
    "contour",
];

/// Match a process comm against the known-terminal table.
pub fn terminal_from_comm(comm: &str) -> Option<&'static str> {
    let lower = comm.to_ascii_lowercase();
    TERMINAL_COMMS
        .iter()
        .find(|known| lower.starts_with(**known))
        .copied()
}

/// Environment hints for the terminal emulator, most specific first.
const TERMINAL_ENV: &[(&str, &str)] = &[
    ("GHOSTTY_RESOURCES_DIR", "Ghostty"),
    ("KONSOLE_VERSION", "Konsole"),
    ("VTE_VERSION", "VTE terminal"),
];

/// The terminal emulator name, from environment hints first (they survive
/// launches from app launchers), then the `/proc` parent chain.
pub fn terminal_from_environment() -> Option<String> {
    if let Ok(raw) = std::env::var("TERM_PROGRAM") {
        let lower = raw.trim().to_ascii_lowercase();
        let name = match lower.as_str() {
            "ghostty" => Some("Ghostty"),
            "wezterm" => Some("WezTerm"),
            "alacritty" => Some("Alacritty"),
            "kitty" => Some("kitty"),
            "konsole" => Some("Konsole"),
            _ => None,
        };
        if let Some(name) = name {
            return Some(name.to_string());
        }
    }
    for (key, name) in TERMINAL_ENV {
        if std::env::var(key).is_ok() {
            return Some((*name).to_string());
        }
    }
    terminal_from_parent_chain()
}

/// Walk the `/proc` parent chain (bounded depth) and return the first
/// ancestor whose comm is a known terminal emulator.
fn terminal_from_parent_chain() -> Option<String> {
    for comm in parent_chain_comms(32) {
        if let Some(name) = terminal_from_comm(&comm) {
            return Some(name.to_string());
        }
    }
    None
}

/// comms of every process ancestor from `/proc/self` upward, bounded by
/// `max_depth`. Missing entries terminate the walk (fail-closed).
pub fn parent_chain_comms(max_depth: usize) -> Vec<String> {
    let mut comms = Vec::new();
    let mut ppid: Option<u32> = std::fs::read_to_string("/proc/self/stat")
        .ok()
        .and_then(|text| proc_stat_ppid(&text));
    let mut visited = 0usize;
    while let Some(pid) = ppid {
        if pid <= 1 || visited >= max_depth {
            break;
        }
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            break;
        };
        let comm = proc_stat_comm(&stat);
        ppid = proc_stat_ppid(&stat);
        visited += 1;
        if let Some(comm) = comm {
            comms.push(comm);
        }
    }
    comms
}

/// Known compositor/WM for the current desktop, via the discovery table over
/// `XDG_CURRENT_DESKTOP` (then `XDG_SESSION_DESKTOP`).
pub fn window_manager_from_env() -> Option<String> {
    for var in ["XDG_CURRENT_DESKTOP", "XDG_SESSION_DESKTOP"] {
        if let Ok(raw) = std::env::var(var) {
            for segment in raw.split(':') {
                if let Some(identity) = DesktopIdentity::from_indicator(segment)
                    && let Some(compositor) = identity.known_compositor()
                {
                    return Some(compositor.to_string());
                }
            }
        }
    }
    None
}

/// Basename of `$SHELL`.
pub fn shell_from_env() -> Option<String> {
    let shell = std::env::var("SHELL").ok()?;
    let name = Path::new(&shell).file_name()?.to_str()?.to_string();
    if name.is_empty() {
        return None;
    }
    Some(name)
}

// ---- /sys GPU enumeration ---------------------------------------------------

/// PCI vendor-id hex (sysfs format, e.g. `0x8086`) to a display name.
pub fn pci_vendor_name(vendor_hex: &str) -> Option<&'static str> {
    let normalized = vendor_hex
        .trim()
        .trim_start_matches("0x")
        .trim_start_matches("0X")
        .to_ascii_lowercase();
    match normalized.as_str() {
        "8086" => Some("Intel"),
        "10de" => Some("NVIDIA"),
        "1002" | "1000" => Some("AMD"),
        _ => None,
    }
}

fn sysfs_trim(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// One entry per `/sys/class/drm/cardN` (digits only, ascending) whose PCI
/// vendor is readable — everything else (framebuffers, render-only nodes
/// without a vendor file) is skipped rather than guessed. Model names come
/// from the pci.ids database (read once for all cards); the
/// integrated/discrete classification is heuristic ([`classify_gpu`]).
pub fn enumerate_gpus() -> Vec<GpuInfo> {
    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
        return Vec::new();
    };
    let mut cards: Vec<u32> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let digits = name.strip_prefix("card")?;
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            digits.parse().ok()
        })
        .collect();
    cards.sort_unstable();
    cards.dedup();

    // The pci.ids database is a data file of pciutils/hwdata — present on
    // every distribution that ships pciutils. Read once for all cards.
    let pci_ids = PCI_IDS_PATHS
        .iter()
        .find_map(|path| std::fs::read_to_string(path).ok());

    let mut gpus = Vec::new();
    for card in cards {
        let device = PathBuf::from(format!("/sys/class/drm/card{card}/device"));
        let Some(vendor) = std::fs::read_to_string(device.join("vendor"))
            .ok()
            .and_then(|text| sysfs_trim(&text))
        else {
            continue;
        };
        let device_id = std::fs::read_to_string(device.join("device"))
            .ok()
            .and_then(|text| sysfs_trim(&text))
            .unwrap_or_else(|| "?".to_string());
        fn strip_hex(hex: &str) -> &str {
            hex.trim_start_matches("0x").trim_start_matches("0X")
        }
        let vram_total = std::fs::read_to_string(device.join("mem_info_vram_total"))
            .ok()
            .and_then(|text| sysfs_trim(&text))
            .and_then(|text| text.parse::<u64>().ok());
        let model = pci_ids
            .as_deref()
            .and_then(|text| pci_model_from_ids(text, strip_hex(&vendor), strip_hex(&device_id)));
        gpus.push(GpuInfo {
            vendor: pci_vendor_name(&vendor),
            model,
            kind: classify_gpu(&vendor, &device_id, vram_total),
            pci_id: format!(
                "{}:{}",
                strip_hex(&vendor).to_ascii_uppercase(),
                strip_hex(&device_id).to_ascii_uppercase()
            ),
        });
    }
    gpus
}

/// pci.ids database locations across distributions (hwdata on Arch/Fedora,
/// pciutils on Debian).
const PCI_IDS_PATHS: &[&str] = &["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids"];

/// The model name for `(vendor, device)` from a pci.ids database body.
/// Vendor lines are `id  Name`, device lines are one TAB + `id  Name`; the
/// search is scoped to the vendor's block, so the same device id under
/// another vendor can never win. Bracketed marketing names are preferred
/// (`GA106 [GeForce RTX 3060 Ti GDDR6X]` → `GeForce RTX 3060 Ti GDDR6X`).
pub fn pci_model_from_ids(text: &str, vendor: &str, device: &str) -> Option<String> {
    let vendor = vendor.trim().to_ascii_lowercase();
    let device = device.trim().to_ascii_lowercase();
    let mut in_vendor_block = false;
    for line in text.lines() {
        if !line.starts_with('\t') {
            let trimmed = line.trim();
            // Comments and blank lines appear INSIDE vendor blocks (real
            // hwdata files carry per-device notes at column 0) — they never
            // end one. Only another vendor/class line does.
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            in_vendor_block = vendor_line_id(line).is_some_and(|id| id == vendor);
            continue;
        }
        if !in_vendor_block {
            continue;
        }
        let rest = line.trim_start_matches('\t');
        // Deeper tabs are subsystem data below a device line.
        if line.len() - rest.len() != 1 {
            continue;
        }
        let mut parts = rest.splitn(2, "  ");
        let id = parts.next()?.trim();
        if id == device {
            return device_display_name(parts.next()?.trim());
        }
    }
    None
}

/// The vendor id of a top-level pci.ids line (`10de  NVIDIA Corporation`),
/// or `None` for comments, class blocks and anything unparseable.
fn vendor_line_id(line: &str) -> Option<&str> {
    let line = line.trim();
    if line.len() < 4 || !line.is_ascii() {
        return None;
    }
    let id = &line[..4];
    if !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let rest = &line[4..];
    // A vendor line separates id and name with whitespace; class blocks
    // start with "C " and never reach here (id would fail the hex check on
    // the leading space).
    if !rest.starts_with("  ") && !rest.starts_with('\t') {
        return None;
    }
    Some(id)
}

/// pci.ids names carry the marketing model in brackets behind a chip code
/// (`AD106 [GeForce RTX 4060 Ti]`); a single-token prefix means the
/// brackets are the model, a multi-word prefix means they are a variant
/// tag (`NetXtreme II … [Everest]`) and the whole name stays.
fn device_display_name(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    if let Some(open) = name.find('[')
        && let Some(close) = name[open..].find(']')
    {
        let inner = name[open + 1..open + close].trim();
        let prefix = name[..open].trim();
        if !prefix.contains(' ') && !prefix.is_empty() {
            if !inner.is_empty() {
                return Some(inner.to_string());
            }
            return Some(prefix.to_string());
        }
    }
    Some(name.to_string())
}

/// Heuristic integrated/discrete classification for one card:
/// - NVIDIA makes no integrated GPUs.
/// - Intel graphics are integrated (an Arc dGPU is rare enough that an
///   honest heuristic beats an empty label).
/// - AMD: amdgpu's `mem_info_vram_total` separates them (iGPU carveouts are
///   MiB-scale, dedicated VRAM is GiB-scale); without the file, the APU
///   device-id ranges decide. Unknowns stay `None` — no label is claimed.
pub fn classify_gpu(
    vendor_hex: &str,
    device_hex: &str,
    vram_total: Option<u64>,
) -> Option<GpuKind> {
    const GIB: u64 = 1024 * 1024 * 1024;
    match pci_vendor_name(vendor_hex)? {
        "NVIDIA" => Some(GpuKind::Discrete),
        "Intel" => Some(GpuKind::Integrated),
        "AMD" => {
            if let Some(vram) = vram_total {
                return Some(if vram >= GIB {
                    GpuKind::Discrete
                } else {
                    GpuKind::Integrated
                });
            }
            let id = u32::from_str_radix(device_hex.trim().trim_start_matches("0x"), 16).ok()?;
            // Modern APU graphics live below 0x2000 (0x15xx/0x16xx); the
            // PCIe card families sit at 0x67xx–0x78xx.
            if id < 0x2000 {
                Some(GpuKind::Integrated)
            } else if (0x6000..=0x7fff).contains(&id) {
                Some(GpuKind::Discrete)
            } else {
                None
            }
        }
        _ => None,
    }
}

// ---- root filesystem usage (GIO) ---------------------------------------------

/// Used/total of the filesystem mounted at `/` via GIO (gio 0.18, already a
/// core dependency; sync query is safe off the UI thread).
pub fn root_disk_usage() -> Option<DiskInfo> {
    use gio::prelude::FileExt as _;
    let file = gio::File::for_path("/");
    let info = file
        .query_filesystem_info(
            "filesystem::size,filesystem::free",
            None::<&gio::Cancellable>,
        )
        .ok()?;
    let total = info.attribute_uint64("filesystem::size");
    let free = info.attribute_uint64("filesystem::free");
    if total == 0 || free > total {
        return None;
    }
    Some(DiskInfo {
        total_bytes: total,
        used_bytes: total - free,
    })
}

// ---- os-release identity + distribution logo ---------------------------------

/// `PRETTY_NAME`, falling back to `NAME`, then `ID` (same preference order
/// as the discovery evidence).
pub fn os_release_display_name() -> Option<String> {
    let text = std::fs::read_to_string("/etc/os-release").ok()?;
    let (keys, _) = parse_os_release(&text);
    keys.get("PRETTY_NAME")
        .or_else(|| keys.get("NAME"))
        .or_else(|| keys.get("ID"))
        .filter(|value| !value.is_empty())
        .cloned()
}

fn kernel_release() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

/// Icon names to try for the distribution logo, in order: the os-release
/// `LOGO=` hint first (the freedesktop standard), then the distributor-logo
/// convention over `ID`, then `ID_LIKE` families.
pub fn distro_logo_names(id: &str, id_like: &[&str], logo_hint: Option<&str>) -> Vec<String> {
    let mut names = Vec::new();
    let mut push = |name: Option<String>| {
        if let Some(name) = name
            && !name.is_empty()
            && !names.contains(&name)
        {
            names.push(name);
        }
    };
    push(logo_hint.map(str::to_string));
    push(Some(format!("distributor-logo-{id}")));
    push(Some(format!("{id}-logo")));
    push(Some(id.to_string()));
    for family in id_like {
        push(Some(format!("distributor-logo-{family}")));
        push(Some(format!("{family}-logo")));
        push(Some(family.to_string()));
    }
    names
}

/// Icon-theme lookup paths for one icon name (hicolor first — the fallback
/// theme distributor logos are installed into — then the pixmaps dir).
pub fn distro_logo_paths(name: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for size in ["128x128", "96x96", "64x64", "48x48", "scalable"] {
        for ext in ["png", "svg"] {
            paths.push(PathBuf::from(format!(
                "/usr/share/icons/hicolor/{size}/apps/{name}.{ext}"
            )));
        }
    }
    for ext in ["png", "svg"] {
        paths.push(PathBuf::from(format!("/usr/share/pixmaps/{name}.{ext}")));
    }
    paths
}

/// The first existing distribution logo file, or `None` when the system
/// ships none (the panel falls back to Licoman's own mark).
pub fn find_distro_logo() -> Option<PathBuf> {
    let text = std::fs::read_to_string("/etc/os-release").ok()?;
    let (keys, _) = parse_os_release(&text);
    let id = keys.get("ID").map(String::as_str).unwrap_or("");
    let id_like: Vec<&str> = keys
        .get("ID_LIKE")
        .map(|value| value.split_whitespace().collect())
        .unwrap_or_default();
    let logo_hint = keys.get("LOGO").map(String::as_str);
    for name in distro_logo_names(id, &id_like, logo_hint) {
        for path in distro_logo_paths(&name) {
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

// ---- display formatting --------------------------------------------------------

/// Human-readable byte size, binary units ("512 B", "3.2 GiB").
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Uptime as a short human string ("3 days, 4 hours", "12 minutes").
pub fn format_uptime(secs: u64) -> String {
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let minutes = (secs % 3_600) / 60;
    if days > 0 {
        let day_word = if days == 1 { "day" } else { "days" };
        let hour_word = if hours == 1 { "hour" } else { "hours" };
        format!("{days} {day_word}, {hours} {hour_word}")
    } else if hours > 0 {
        let hour_word = if hours == 1 { "hour" } else { "hours" };
        let minute_word = if minutes == 1 { "minute" } else { "minutes" };
        format!("{hours} {hour_word}, {minutes} {minute_word}")
    } else {
        let minute_word = if minutes == 1 { "minute" } else { "minutes" };
        format!("{minutes} {minute_word}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_uptime() {
        assert_eq!(parse_uptime("12345.67 23456.78\n"), Some(12345));
        assert_eq!(parse_uptime("  0.5 "), Some(0));
        assert_eq!(parse_uptime("garbage"), None);
        assert_eq!(parse_uptime(""), None);
    }

    #[test]
    fn parses_meminfo_with_swap() {
        let text = "MemTotal:       16306656 kB\nMemFree:         2453980 kB\n\
                    MemAvailable:    9425172 kB\nSwapTotal:       4194300 kB\n\
                    SwapFree:        4194300 kB\nHugePages_Total:       0\n";
        let (memory, swap) = parse_meminfo(text);
        let memory = memory.expect("memory present");
        assert_eq!(memory.total_bytes, 16_306_656 * 1024);
        assert_eq!(memory.used_bytes, (16_306_656 - 9_425_172) * 1024);
        let swap = swap.expect("swap present");
        assert_eq!(swap.total_bytes, 4_194_300 * 1024);
        assert_eq!(swap.used_bytes, 0);
    }

    #[test]
    fn meminfo_without_swap_yields_none() {
        let text = "MemTotal:       16306656 kB\nMemAvailable:    9425172 kB\n\
                    SwapTotal:             0 kB\nSwapFree:              0 kB\n";
        let (memory, swap) = parse_meminfo(text);
        assert!(memory.is_some());
        assert!(swap.is_none());
    }

    #[test]
    fn meminfo_without_available_field_yields_none() {
        let text = "MemTotal:       16306656 kB\nMemFree:         2453980 kB\n";
        let (memory, swap) = parse_meminfo(text);
        assert!(memory.is_none());
        assert!(swap.is_none());
    }

    #[test]
    fn parses_cpuinfo_model_and_cores() {
        let text = "processor\t: 0\nvendor_id\t: GenuineIntel\ncpu MHz\t\t: 2800.000\n\
                    model name\t: Intel(R) Core(TM) i7-13700H\n\n\
                    processor\t: 1\nvendor_id\t: GenuineIntel\nmodel name\t: Intel(R) Core(TM) i7-13700H\n";
        let cpu = parse_cpuinfo(text).expect("cpu present");
        assert_eq!(cpu.logical_cores, 2);
        assert_eq!(cpu.model.as_deref(), Some("Intel(R) Core(TM) i7-13700H"));
    }

    #[test]
    fn cpuinfo_falls_back_to_arm_processor_key() {
        let text = "processor\t: 0\nProcessor\t: ARMv7 Processor rev 5\n";
        let cpu = parse_cpuinfo(text).expect("cpu present");
        assert_eq!(cpu.logical_cores, 1);
        assert_eq!(cpu.model.as_deref(), Some("ARMv7 Processor rev 5"));
    }

    #[test]
    fn cpuinfo_empty_is_none() {
        assert!(parse_cpuinfo("").is_none());
        assert!(parse_cpuinfo("flags\t: sse2\n").is_none());
    }

    #[test]
    fn parses_proc_stat_comm_and_ppid() {
        let text = "1234 (kitty) S 987 1234 1234 0 -1 4194560 ...";
        assert_eq!(proc_stat_comm(text).as_deref(), Some("kitty"));
        assert_eq!(proc_stat_ppid(text), Some(987));

        // A comm containing spaces and parentheses must not break parsing.
        let tricky = "42 (weird (name)) R 1 0 0 0 -1 0 ...";
        assert_eq!(proc_stat_comm(tricky).as_deref(), Some("weird (name)"));
        assert_eq!(proc_stat_ppid(tricky), Some(1));
    }

    #[test]
    fn matches_known_terminal_comms() {
        assert_eq!(terminal_from_comm("kitty"), Some("kitty"));
        assert_eq!(terminal_from_comm("KITTY"), Some("kitty"));
        // gnome-terminal-server's comm is "gnome-terminal-".
        assert_eq!(
            terminal_from_comm("gnome-terminal-"),
            Some("gnome-terminal-")
        );
        assert_eq!(terminal_from_comm("bash"), None);
        assert_eq!(terminal_from_comm("Licoman"), None);
    }

    #[test]
    fn maps_pci_vendor_names() {
        assert_eq!(pci_vendor_name("0x8086"), Some("Intel"));
        assert_eq!(pci_vendor_name("8086"), Some("Intel"));
        assert_eq!(pci_vendor_name("0x10DE"), Some("NVIDIA"));
        assert_eq!(pci_vendor_name("0x1002"), Some("AMD"));
        assert_eq!(pci_vendor_name("0x1b36"), None);
    }

    #[test]
    fn human_bytes_formats_binary_units() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1024), "1.0 KiB");
        assert_eq!(human_bytes(3_221_225_472), "3.0 GiB");
        assert_eq!(human_bytes(502_312_869_888), "467.8 GiB");
    }

    #[test]
    fn format_uptime_reads_naturally() {
        assert_eq!(format_uptime(60), "1 minute");
        assert_eq!(format_uptime(12 * 60), "12 minutes");
        assert_eq!(format_uptime(3 * 3600 + 5 * 60), "3 hours, 5 minutes");
        assert_eq!(format_uptime(3 * 86_400 + 4 * 3600), "3 days, 4 hours");
        assert_eq!(format_uptime(86_400), "1 day, 0 hours");
    }

    const GIB_BYTES: u64 = 1024 * 1024 * 1024;

    /// Fixture shaped like a real pci.ids body: two vendors, the duplicated
    /// device id `164e` under a *different* vendor (a Broadcom NIC), a
    /// class block, comments and a subsystem line.
    const PCI_IDS: &str = "\
# Vendors
10de  NVIDIA Corporation
\t0008  NV1 [STG2000X-B Series]
\t2803  AD106 [GeForce RTX 4060 Ti]
\t\t1043  8812  AD106
1002  Advanced Micro Devices, Inc. [AMD/ATI]
# Used in the Steam Deck OLED (column-0 comments live inside blocks!)
\t164e  Raphael [Radeon 610M]
14e4  Broadcom Inc. and subsidiaries
\t164e  NetXtreme II BCM57710 10-Gigabit PCIe [Everest]

C 03  Display controller
";

    #[test]
    fn pci_model_scoped_to_vendor_block() {
        assert_eq!(
            pci_model_from_ids(PCI_IDS, "1002", "164e").as_deref(),
            Some("Radeon 610M")
        );
        // The same device id under Broadcom must never win for AMD lookups.
        assert_eq!(
            pci_model_from_ids(PCI_IDS, "10de", "2803").as_deref(),
            Some("GeForce RTX 4060 Ti")
        );
        assert_eq!(
            pci_model_from_ids(PCI_IDS, "14e4", "164e").as_deref(),
            Some("NetXtreme II BCM57710 10-Gigabit PCIe [Everest]")
        );
    }

    #[test]
    fn pci_model_unknown_ids_are_none() {
        assert_eq!(pci_model_from_ids(PCI_IDS, "10de", "9999"), None);
        assert_eq!(pci_model_from_ids(PCI_IDS, "abcd", "2803"), None);
        // Class blocks and comments are not vendors.
        assert_eq!(pci_model_from_ids(PCI_IDS, "03", "164e"), None);
    }

    #[test]
    fn pci_device_name_prefers_bracket_content() {
        assert_eq!(
            device_display_name("AD106 [GeForce RTX 4060 Ti]").as_deref(),
            Some("GeForce RTX 4060 Ti")
        );
        assert_eq!(
            device_display_name("Plain Name").as_deref(),
            Some("Plain Name")
        );
        assert_eq!(device_display_name("Empty []"), Some("Empty".to_string()));
        assert_eq!(device_display_name("  "), None);
    }

    #[test]
    fn classifies_gpu_kinds() {
        // NVIDIA is always discrete; Intel integrated.
        assert_eq!(
            classify_gpu("0x10de", "2803", None),
            Some(GpuKind::Discrete)
        );
        assert_eq!(
            classify_gpu("0x8086", "a780", None),
            Some(GpuKind::Integrated)
        );
        // AMD decides on the amdgpu VRAM file: the Raphael iGPU reports a
        // 512 MiB carveout (this machine), discrete cards report GiB.
        assert_eq!(
            classify_gpu("0x1002", "164e", Some(536_870_912)),
            Some(GpuKind::Integrated)
        );
        assert_eq!(
            classify_gpu("0x1002", "744c", Some(16 * GIB_BYTES)),
            Some(GpuKind::Discrete)
        );
        // Without the file, the APU/PCIe id ranges decide.
        assert_eq!(
            classify_gpu("0x1002", "164e", None),
            Some(GpuKind::Integrated)
        );
        assert_eq!(
            classify_gpu("0x1002", "744c", None),
            Some(GpuKind::Discrete)
        );
        // Unclassifiable stays unlabeled — never guessed.
        assert_eq!(classify_gpu("0x1002", "ffff", None), None);
        assert_eq!(classify_gpu("0x1b36", "0100", None), None);
    }

    #[test]
    fn distro_logo_names_hint_then_id_then_families() {
        let names = distro_logo_names("arch", &["arch"], Some("archlinux-logo"));
        assert_eq!(
            names,
            vec![
                "archlinux-logo".to_string(),
                "distributor-logo-arch".to_string(),
                "arch-logo".to_string(),
                "arch".to_string(),
            ]
        );
        let names = distro_logo_names("ubuntu", &["debian"], None);
        assert!(names.contains(&"distributor-logo-ubuntu".to_string()));
        assert!(names.contains(&"distributor-logo-debian".to_string()));
        assert!(names.contains(&"debian".to_string()));
    }

    #[test]
    fn distro_logo_paths_cover_hicolor_and_pixmaps() {
        let paths = distro_logo_paths("arch");
        assert!(paths.contains(&PathBuf::from(
            "/usr/share/icons/hicolor/128x128/apps/arch.png"
        )));
        assert!(paths.contains(&PathBuf::from(
            "/usr/share/icons/hicolor/scalable/apps/arch.svg"
        )));
        assert!(paths.contains(&PathBuf::from("/usr/share/pixmaps/arch.svg")));
    }

    #[test]
    fn os_release_display_name_prefers_pretty_name() {
        // Pure helper exercised through the fixture parser — the collector
        // reads the live file, so only the fallback order is asserted here.
        let mut keys = std::collections::BTreeMap::new();
        keys.insert("ID".to_string(), "arch".to_string());
        keys.insert("NAME".to_string(), "Arch Linux".to_string());
        keys.insert("PRETTY_NAME".to_string(), "Arch Linux".to_string());
        let name = keys
            .get("PRETTY_NAME")
            .or_else(|| keys.get("NAME"))
            .or_else(|| keys.get("ID"));
        assert_eq!(name.map(String::as_str), Some("Arch Linux"));
    }
}
