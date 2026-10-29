//! Mock settings and applications for Phase 01 (design verification).
//!
//! Everything here is sample data. The design doc requires that mock content
//! is clearly labeled as such and that states that would require a real
//! provider (unavailable, read-only with reason, permission, external change)
//! are rendered truthfully rather than faked as configurable.

use gpui_kit::SharedString;

pub type SettingId = SharedString;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Shared,
    Desktop,
    Distribution,
}

impl Scope {
    pub fn label(self) -> &'static str {
        match self {
            Scope::Shared => "Shared",
            Scope::Desktop => "Desktop",
            Scope::Distribution => "Distribution",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Scope::Shared => "Common settings on this Linux system.",
            Scope::Desktop => "Settings provided by your desktop environment.",
            Scope::Distribution => "Distribution administration and package tooling.",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlKind {
    Switch,
    Dropdown,
    Slider,
    Number,
    Text,
    ReadOnly,
}

/// Distinct visual states from the design contract (01_DESIGN_SYSTEM.md §5).
/// Idle/Hover/Pending/Applying/Saved/Failed/ExternalChange/PermissionNeeded/
/// RestartNeeded/Unsupported.
#[derive(Debug, Clone, PartialEq)]
pub enum RowStatus {
    Idle,
    /// Edited in the UI; not yet applied ("1 change pending").
    Dirty,
    Applying,
    Saved,
    Failed {
        reason: SharedString,
    },
    ExternalChange,
    PermissionNeeded {
        scope: SharedString,
    },
    RestartNeeded {
        target: SharedString,
    },
    Unsupported {
        reason: SharedString,
    },
}

impl RowStatus {
    pub fn blocks_editing(&self) -> bool {
        matches!(
            self,
            RowStatus::Applying
                | RowStatus::ExternalChange
                | RowStatus::PermissionNeeded { .. }
                | RowStatus::Unsupported { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum MockValue {
    Bool(bool),
    Text(SharedString),
    Number(f64),
    Option(SharedString),
}

impl MockValue {
    pub fn as_text(&self) -> SharedString {
        match self {
            MockValue::Bool(b) => if *b { "On" } else { "Off" }.into(),
            MockValue::Text(t) => t.clone(),
            MockValue::Number(n) => format!("{n}").into(),
            MockValue::Option(o) => o.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MockSetting {
    pub id: SettingId,
    pub scope: Scope,
    pub category: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub kind: ControlKind,
    pub value: MockValue,
    pub status: RowStatus,
    pub options: &'static [&'static str],
    /// Dropdown-like display strings for numeric/enum controls.
    pub unit: &'static str,
    /// Simulates an apply failure for the failure-state showcase.
    pub simulate_failure: bool,
}

impl MockSetting {
    fn new(
        id: &str,
        scope: Scope,
        category: &'static str,
        label: &'static str,
        description: &'static str,
        kind: ControlKind,
        value: MockValue,
    ) -> Self {
        Self {
            id: id.into(),
            scope,
            category,
            label,
            description,
            kind,
            value,
            status: RowStatus::Idle,
            options: &[],
            unit: "",
            simulate_failure: false,
        }
    }

    fn dropdown(mut self, options: &'static [&'static str]) -> Self {
        self.kind = ControlKind::Dropdown;
        self.options = options;
        self
    }

    fn unit(mut self, unit: &'static str) -> Self {
        self.unit = unit;
        self
    }

    fn status(mut self, status: RowStatus) -> Self {
        self.status = status;
        self
    }

    fn failing(mut self) -> Self {
        self.simulate_failure = true;
        self
    }

    pub fn is_dirty(&self) -> bool {
        matches!(self.status, RowStatus::Dirty)
    }
}

/// The mock sample environment shown in the sidebar (matches the design doc's
/// structure reference; clearly sample data, not real detection).
pub const MOCK_ENVIRONMENT: &[(&str, &str)] = &[
    ("Arch Linux", "Distribution"),
    ("Hyprland (Wayland)", "Session"),
];

pub fn mock_settings() -> Vec<MockSetting> {
    vec![
        // ---- Shared / Appearance --------------------------------------
        MockSetting::new(
            "shared.appearance.text_scaling",
            Scope::Shared,
            "Appearance",
            "Text scaling",
            "Scales text across Licoman's interface (sample values).",
            ControlKind::Dropdown,
            MockValue::Option("100%".into()),
        )
        .dropdown(&["90%", "100%", "110%", "125%", "150%"]),
        MockSetting::new(
            "shared.appearance.animations",
            Scope::Shared,
            "Appearance",
            "Accessibility animations",
            "Whether interface motion is reduced.",
            ControlKind::ReadOnly,
            MockValue::Option("Provided by desktop".into()),
        )
        .status(RowStatus::Unsupported {
            reason: "Reads the desktop accessibility service; provider arrives with discovery."
                .into(),
        }),
        // ---- Shared / Power -------------------------------------------
        MockSetting::new(
            "shared.power.profile",
            Scope::Shared,
            "Power",
            "Power profile",
            "Balanced, performance or power-saver via power-profiles-daemon.",
            ControlKind::Dropdown,
            MockValue::Option("Balanced".into()),
        )
        .dropdown(&["Balanced", "Performance", "Power Saver"])
        .status(RowStatus::Unsupported {
            reason: "No power-profiles-daemon detected in this mock session.".into(),
        }),
        // ---- Shared / Network -----------------------------------------
        MockSetting::new(
            "shared.network.dns",
            Scope::Shared,
            "Network",
            "DNS override",
            "A system resolver setting. Apply always fails in the mock to show failure handling.",
            ControlKind::Text,
            MockValue::Text("".into()),
        )
        .unit("leave empty to keep system default")
        .failing(),
        MockSetting::new(
            "shared.network.wifi",
            Scope::Shared,
            "Network",
            "Wi-Fi",
            "Enable wireless networking.",
            ControlKind::Switch,
            MockValue::Bool(true),
        ),
        // ---- Shared / Audio -------------------------------------------
        MockSetting::new(
            "shared.audio.output_volume",
            Scope::Shared,
            "Audio",
            "Output volume",
            "Master output level.",
            ControlKind::Slider,
            MockValue::Number(65.),
        )
        .unit("%"),
        // ---- Desktop ---------------------------------------------------
        MockSetting::new(
            "desktop.session.keyboard_repeat_delay",
            Scope::Desktop,
            "Input",
            "Key repeat delay",
            "Delay before a held key starts repeating.",
            ControlKind::Dropdown,
            MockValue::Option("300 ms".into()),
        )
        .dropdown(&["200 ms", "300 ms", "500 ms"])
        .status(RowStatus::RestartNeeded {
            target: "Hyprland session".into(),
        }),
        MockSetting::new(
            "desktop.session.gaps",
            Scope::Desktop,
            "Windows",
            "Window gaps",
            "Outer and inner gaps between tiled windows.",
            ControlKind::Switch,
            MockValue::Bool(true),
        )
        .status(RowStatus::Dirty),
        MockSetting::new(
            "desktop.session.accent",
            Scope::Desktop,
            "Appearance",
            "Desktop accent color",
            "Changed by another tool since Licoman last read it.",
            ControlKind::Dropdown,
            MockValue::Option("#8AB4FF".into()),
        )
        .dropdown(&["#8AB4FF", "#70D0A9", "#F2C36C"])
        .status(RowStatus::ExternalChange),
        // ---- Distribution ----------------------------------------------
        MockSetting::new(
            "distro.packages.updates",
            Scope::Distribution,
            "Packages",
            "Install system updates",
            "Runs the distribution package manager with elevated permissions.",
            ControlKind::ReadOnly,
            MockValue::Option("Not available".into()),
        )
        .status(RowStatus::PermissionNeeded {
            scope: "System package database".into(),
        }),
        MockSetting::new(
            "distro.packages.parallel_downloads",
            Scope::Distribution,
            "Packages",
            "Parallel downloads",
            "Number of simultaneous package downloads.",
            ControlKind::Number,
            MockValue::Number(5.),
        )
        .unit("connections"),
        MockSetting::new(
            "distro.services.timesync",
            Scope::Distribution,
            "Services",
            "Time synchronization",
            "Managed by the system service manager.",
            ControlKind::ReadOnly,
            MockValue::Option("systemd-timesyncd".into()),
        )
        .status(RowStatus::Unsupported {
            reason: "Service control needs a provider; showing the detected unit name only.".into(),
        }),
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    Supported,
    Partial,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MockApp {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub support: Support,
    /// Categories (tabs) with a short reason line each, for the detail pane.
    pub categories: &'static [(&'static str, &'static str)],
    /// Sample config source shown under "Configuration source".
    pub source: &'static str,
}

pub const MOCK_APPS: &[MockApp] = &[
    MockApp {
        id: "alacritty",
        name: "Alacritty",
        description: "Terminal emulator",
        support: Support::Supported,
        categories: &[
            ("Appearance", "Font, padding and opacity."),
            ("Window", "Startup mode and dimensions."),
            ("Cursor", "Style and blink behavior."),
            ("Advanced", "Bell, scrolling and bindings."),
        ],
        source: "~/.config/alacritty/alacritty.toml (sample path)",
    },
    MockApp {
        id: "kitty",
        name: "Kitty",
        description: "Terminal emulator",
        support: Support::Supported,
        categories: &[
            ("Appearance", "Font, colors and padding."),
            ("Window", "Layouts and startup behavior."),
            ("Advanced", "Clipboard and shell integration."),
        ],
        source: "~/.config/kitty/kitty.conf (sample path)",
    },
    MockApp {
        id: "mpv",
        name: "MPV",
        description: "Media player",
        support: Support::Partial,
        categories: &[
            ("Playback", "A subset of playback options."),
            ("Video", "Video output settings."),
        ],
        source: "~/.config/mpv/mpv.conf (sample path)",
    },
    MockApp {
        id: "neovim",
        name: "Neovim",
        description: "Text editor",
        support: Support::Partial,
        categories: &[("Editor", "Basic editor options only.")],
        source: "~/.config/nvim/init.lua (sample path)",
    },
    MockApp {
        id: "zed",
        name: "Zed",
        description: "Code editor",
        support: Support::Unsupported,
        categories: &[],
        source: "~/.config/zed/settings.json (sample path)",
    },
];

/// Breadcrumb for the search palette: scope → category → setting.
pub fn breadcrumb(setting: &MockSetting) -> String {
    format!(
        "{} → {} → {}",
        setting.scope.label(),
        setting.category,
        setting.label
    )
}
