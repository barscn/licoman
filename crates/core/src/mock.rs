//! Deterministic mock provider: drives the store lifecycle tests — search,
//! dirty/apply/failure/conflict transitions — without any installed software
//! (02_FOUNDATION.md §2). Since Phase 04 the *shipped* catalog comes from
//! module manifests ([`crate::modules`]); this hand-written provider remains
//! as the lifecycle-test fixture and is not registered by the app.
//!
//! Determinism: reads return values from a per-setting sequence (so an
//! external change can be simulated as "the source changed between our
//! reads"), writes are synchronous, and failures are declared per fixture.
//! Everything visible is mock content and the UI labels it as such.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::descriptor::{Control, Scope, SettingDescriptor};
use crate::error::{ErrorKind, LicomanError};
use crate::provider::{Capabilities, ProbeResult, Provider};
use crate::value::{SettingType, SettingValue};

/// One mock setting: its descriptor plus the deterministic behavior script.
#[derive(Debug, Clone)]
pub struct MockSettingFixture {
    pub descriptor: SettingDescriptor,
    /// Values returned by successive reads until a write happens (last one
    /// repeats). A two-entry sequence simulates "changed outside Licoman".
    pub reads: Vec<SettingValue>,
    pub unavailable_reason: Option<&'static str>,
    pub fail_writes: bool,
}

impl MockSettingFixture {
    pub fn new(descriptor: SettingDescriptor, initial: SettingValue) -> Self {
        Self {
            descriptor,
            reads: vec![initial],
            unavailable_reason: None,
            fail_writes: false,
        }
    }

    /// The source value changes after the first read: the initial load sees
    /// the old value, the verification pass sees the new one, which the store
    /// reports as an external conflict.
    pub fn then_source_becomes(mut self, changed: SettingValue) -> Self {
        self.reads.push(changed);
        self
    }

    /// Reading fails: the setting cannot work in this environment.
    pub fn unavailable(mut self, reason: &'static str) -> Self {
        self.unavailable_reason = Some(reason);
        self
    }

    /// Writes are rejected (failure-state showcase and tests).
    pub fn fail_writes(mut self) -> Self {
        self.fail_writes = true;
        self
    }
}

struct MockRuntime {
    read_counts: HashMap<String, usize>,
    written: HashMap<String, SettingValue>,
}

/// A [`Provider`] over declared fixtures. Available in every environment by
/// definition; per-setting states come from the fixtures.
pub struct MockProvider {
    id: &'static str,
    fixtures: Vec<MockSettingFixture>,
    runtime: Mutex<MockRuntime>,
}

impl MockProvider {
    pub fn new(id: &'static str, fixtures: Vec<MockSettingFixture>) -> Self {
        Self {
            id,
            fixtures,
            runtime: Mutex::new(MockRuntime {
                read_counts: HashMap::new(),
                written: HashMap::new(),
            }),
        }
    }

    /// The Phase 01 sample catalog, ported to descriptors and typed values.
    /// IDs are unchanged from the design phase (stable identities).
    pub fn standard() -> Self {
        let fixtures = vec![
            // ---- Shared / Appearance -----------------------------------
            MockSettingFixture::new(
                SettingDescriptor::new(
                    "shared.appearance.text_scaling",
                    Scope::Shared,
                    "Appearance",
                    "Text scaling",
                    "Scales text across Licoman's interface (sample values).",
                    SettingType::Choice,
                    Control::Choice {
                        options: ["90%", "100%", "110%", "125%", "150%"]
                            .iter()
                            .map(|s| s.to_string())
                            .collect(),
                    },
                ),
                SettingValue::Choice("100%".into()),
            ),
            MockSettingFixture::new(
                SettingDescriptor::new(
                    "shared.appearance.animations",
                    Scope::Shared,
                    "Appearance",
                    "Accessibility animations",
                    "Whether interface motion is reduced.",
                    SettingType::Text,
                    Control::ReadOnly,
                )
                .apply(|d| {
                    d.note = Some(
                        "Reads the desktop accessibility service; provider arrives with discovery.".into(),
                    );
                }),
                SettingValue::Text("Provided by desktop".into()),
            ),
            // ---- Shared / Power ----------------------------------------
            MockSettingFixture::new(
                SettingDescriptor::new(
                    "shared.power.profile",
                    Scope::Shared,
                    "Power",
                    "Power profile",
                    "Balanced, performance or power-saver via power-profiles-daemon.",
                    SettingType::Choice,
                    Control::Choice {
                        options: ["Balanced", "Performance", "Power Saver"]
                            .iter()
                            .map(|s| s.to_string())
                            .collect(),
                    },
                ),
                SettingValue::Choice("Balanced".into()),
            )
            .unavailable("No power-profiles-daemon detected in this mock session."),
            // ---- Shared / Network --------------------------------------
            MockSettingFixture::new(
                SettingDescriptor::new(
                    "shared.network.dns",
                    Scope::Shared,
                    "Network",
                    "DNS override",
                    "A system resolver setting. Apply always fails in the mock to show failure handling.",
                    SettingType::Text,
                    Control::Text {
                        placeholder: "Leave empty to keep system default".into(),
                    },
                ),
                SettingValue::Text(String::new()),
            )
            .fail_writes(),
            MockSettingFixture::new(
                SettingDescriptor::new(
                    "shared.network.wifi",
                    Scope::Shared,
                    "Network",
                    "Wi-Fi",
                    "Enable wireless networking.",
                    SettingType::Bool,
                    Control::Switch,
                ),
                SettingValue::Bool(true),
            ),
            // ---- Shared / Audio ----------------------------------------
            MockSettingFixture::new(
                SettingDescriptor::new(
                    "shared.audio.output_volume",
                    Scope::Shared,
                    "Audio",
                    "Output volume",
                    "Master output level.",
                    SettingType::Float,
                    Control::Slider {
                        step: 1.,
                        unit: "%".into(),
                    },
                )
                .apply(|d| {
                    d.min = Some(0.);
                    d.max = Some(100.);
                }),
                SettingValue::Float(65.),
            ),
            // ---- Desktop ------------------------------------------------
            MockSettingFixture::new(
                SettingDescriptor::new(
                    "desktop.session.keyboard_repeat_delay",
                    Scope::Desktop,
                    "Input",
                    "Key repeat delay",
                    "Delay before a held key starts repeating.",
                    SettingType::Choice,
                    Control::Choice {
                        options: ["200 ms", "300 ms", "500 ms"]
                            .iter()
                            .map(|s| s.to_string())
                            .collect(),
                    },
                )
                .apply(|d| {
                    d.restart_target = Some("Hyprland session".into());
                }),
                SettingValue::Choice("300 ms".into()),
            ),
            MockSettingFixture::new(
                SettingDescriptor::new(
                    "desktop.session.gaps",
                    Scope::Desktop,
                    "Windows",
                    "Window gaps",
                    "Outer and inner gaps between tiled windows.",
                    SettingType::Bool,
                    Control::Switch,
                ),
                SettingValue::Bool(true),
            ),
            MockSettingFixture::new(
                SettingDescriptor::new(
                    "desktop.session.accent",
                    Scope::Desktop,
                    "Appearance",
                    "Desktop accent color",
                    "Changed by another tool since Licoman last read it.",
                    SettingType::Choice,
                    Control::Choice {
                        options: ["#8AB4FF", "#70D0A9", "#F2C36C"]
                            .iter()
                            .map(|s| s.to_string())
                            .collect(),
                    },
                ),
                // First read: what Licoman "knew"; second read: the source
                // meanwhile changed — the store reports an external conflict.
                SettingValue::Choice("#70D0A9".into()),
            )
            .then_source_becomes(SettingValue::Choice("#8AB4FF".into())),
            // ---- Distribution -------------------------------------------
            MockSettingFixture::new(
                SettingDescriptor::new(
                    "distro.packages.updates",
                    Scope::Distribution,
                    "Packages",
                    "Install system updates",
                    "Runs the distribution package manager with elevated permissions.",
                    SettingType::Text,
                    Control::ReadOnly,
                )
                .apply(|d| {
                    d.permission_scope = Some("System package database".into());
                }),
                SettingValue::Text("Not available".into()),
            ),
            MockSettingFixture::new(
                SettingDescriptor::new(
                    "distro.packages.parallel_downloads",
                    Scope::Distribution,
                    "Packages",
                    "Parallel downloads",
                    "Number of simultaneous package downloads.",
                    SettingType::Integer,
                    Control::Number {
                        step: 1.,
                        unit: "connections".into(),
                    },
                )
                .apply(|d| {
                    d.min = Some(0.);
                    d.max = Some(64.);
                }),
                SettingValue::Integer(5),
            ),
            MockSettingFixture::new(
                SettingDescriptor::new(
                    "distro.services.timesync",
                    Scope::Distribution,
                    "Services",
                    "Time synchronization",
                    "Managed by the system service manager.",
                    SettingType::Text,
                    Control::ReadOnly,
                )
                .apply(|d| {
                    d.note =
                        Some("Service control needs a provider; showing the detected unit name only.".into());
                }),
                SettingValue::Text("systemd-timesyncd".into()),
            ),
        ];
        MockProvider::new("mock-standard", fixtures)
    }

    fn fixture(&self, id: &str) -> Option<&MockSettingFixture> {
        self.fixtures.iter().find(|f| f.descriptor.id == id)
    }
}

impl Provider for MockProvider {
    fn id(&self) -> &'static str {
        self.id
    }

    fn probe(&self) -> ProbeResult {
        ProbeResult::available(Capabilities {
            read: true,
            write: true,
            watch: false,
        })
    }

    fn descriptors(&self) -> Vec<SettingDescriptor> {
        self.fixtures.iter().map(|f| f.descriptor.clone()).collect()
    }

    fn read(&self, id: &str) -> Result<SettingValue, LicomanError> {
        let fixture = self.fixture(id).ok_or_else(|| {
            LicomanError::new(
                ErrorKind::Unavailable,
                "This setting is not registered.",
                format!("mock provider has no fixture for id {id:?}"),
            )
        })?;
        if let Some(reason) = fixture.unavailable_reason {
            return Err(LicomanError::new(
                ErrorKind::Unavailable,
                reason,
                format!("mock fixture marks {id:?} unavailable"),
            ));
        }
        if let Some(scope) = &fixture.descriptor.permission_scope {
            return Err(LicomanError::new(
                ErrorKind::Permission,
                format!("Requires permission — {scope}"),
                format!("mock fixture marks {id:?} permission-held"),
            ));
        }

        let mut runtime = self.runtime.lock().expect("mock runtime lock");
        if let Some(written) = runtime.written.get(id) {
            return Ok(written.clone());
        }
        let count = runtime.read_counts.entry(id.to_string()).or_insert(0);
        let index = (*count).min(fixture.reads.len() - 1);
        *count += 1;
        Ok(fixture.reads[index].clone())
    }

    fn write(&self, id: &str, value: &SettingValue) -> Result<(), LicomanError> {
        let fixture = self.fixture(id).ok_or_else(|| {
            LicomanError::new(
                ErrorKind::Unavailable,
                "This setting is not registered.",
                format!("mock provider has no fixture for id {id:?}"),
            )
        })?;
        if let Some(reason) = fixture.unavailable_reason {
            return Err(LicomanError::new(
                ErrorKind::Unavailable,
                reason,
                "mock fixture is unavailable; writes rejected".to_string(),
            ));
        }
        if fixture.descriptor.permission_scope.is_some() {
            return Err(LicomanError::new(
                ErrorKind::Permission,
                "Requires permission — this build cannot authorize privileged writes.",
                "mock fixture is permission-held".to_string(),
            ));
        }
        if fixture.fail_writes {
            return Err(LicomanError::new(
                ErrorKind::WriteFailed,
                "The mock provider rejected this write.",
                format!("mock fixture for {id:?} is configured to fail writes"),
            ));
        }
        if fixture.descriptor.control == Control::ReadOnly {
            return Err(LicomanError::new(
                ErrorKind::WriteFailed,
                "This setting is read-only in this environment.",
                "mock fixture control is read-only".to_string(),
            ));
        }
        self.runtime
            .lock()
            .expect("mock runtime lock")
            .written
            .insert(id.to_string(), value.clone());
        Ok(())
    }
}

/// Helper used by fixture construction: apply a mutation to the descriptor.
trait ApplyOn {
    fn apply(self, f: impl FnOnce(&mut SettingDescriptor)) -> Self;
}

impl ApplyOn for SettingDescriptor {
    fn apply(mut self, f: impl FnOnce(&mut SettingDescriptor)) -> Self {
        f(&mut self);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_fixtures_have_unique_stable_ids() {
        let provider = MockProvider::standard();
        let ids: Vec<String> = provider
            .fixtures
            .iter()
            .map(|f| f.descriptor.id.clone())
            .collect();
        for (i, a) in ids.iter().enumerate() {
            for b in ids.iter().skip(i + 1) {
                assert_ne!(a, b);
            }
        }
        assert_eq!(ids.len(), 12);
    }

    #[test]
    fn read_sequence_advances_then_repeats_until_write() {
        let provider = MockProvider::new(
            "t",
            vec![
                MockSettingFixture::new(
                    SettingDescriptor::new(
                        "t.x",
                        Scope::Shared,
                        "C",
                        "X",
                        "",
                        SettingType::Bool,
                        Control::Switch,
                    ),
                    SettingValue::Bool(false),
                )
                .then_source_becomes(SettingValue::Bool(true)),
            ],
        );
        assert_eq!(provider.read("t.x").unwrap(), SettingValue::Bool(false));
        assert_eq!(provider.read("t.x").unwrap(), SettingValue::Bool(true));
        // Stays at the last value of the sequence.
        assert_eq!(provider.read("t.x").unwrap(), SettingValue::Bool(true));
        provider.write("t.x", &SettingValue::Bool(false)).unwrap();
        // After a write, reads report the written value.
        assert_eq!(provider.read("t.x").unwrap(), SettingValue::Bool(false));
    }

    #[test]
    fn unavailable_and_permission_fixtures_fail_closed() {
        let provider = MockProvider::new(
            "t",
            vec![
                MockSettingFixture::new(
                    SettingDescriptor::new(
                        "t.off",
                        Scope::Shared,
                        "C",
                        "Off",
                        "",
                        SettingType::Bool,
                        Control::Switch,
                    ),
                    SettingValue::Bool(false),
                )
                .unavailable("gone"),
                MockSettingFixture::new(
                    SettingDescriptor::new(
                        "t.perm",
                        Scope::Shared,
                        "C",
                        "Perm",
                        "",
                        SettingType::Text,
                        Control::ReadOnly,
                    )
                    .apply(|d| d.permission_scope = Some("Root".into())),
                    SettingValue::Text("x".into()),
                ),
            ],
        );
        assert_eq!(
            provider.read("t.off").unwrap_err().kind,
            ErrorKind::Unavailable
        );
        assert_eq!(
            provider.read("t.perm").unwrap_err().kind,
            ErrorKind::Permission
        );
        assert!(provider.write("t.off", &SettingValue::Bool(true)).is_err());
    }
}
