//! Typed setting values (02_FOUNDATION.md §2): bool, string, integer, float,
//! enum (choice), and the optional-capable path/list types. `Path` and `List`
//! model optional data through an empty value ("not set") so a missing path is
//! a value, not an error.

use crate::descriptor::SettingDescriptor;
use crate::error::{ErrorKind, LicomanError};

/// The value type of a setting, as declared by its descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingType {
    Bool,
    Text,
    Integer,
    Float,
    /// Selection from a fixed option list.
    Choice,
    /// Optional filesystem path; empty string = not set.
    Path,
    /// Optional list of strings; empty list = not set.
    List,
}

impl SettingType {
    pub fn as_str(self) -> &'static str {
        match self {
            SettingType::Bool => "bool",
            SettingType::Text => "string",
            SettingType::Integer => "integer",
            SettingType::Float => "float",
            SettingType::Choice => "enum",
            SettingType::Path => "path",
            SettingType::List => "list",
        }
    }
}

/// A concrete setting value.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingValue {
    Bool(bool),
    Text(String),
    Integer(i64),
    Float(f64),
    /// The selected option; must be one of the descriptor's options.
    Choice(String),
    /// Optional filesystem path; empty = not set.
    Path(String),
    /// Optional list; empty = not set.
    List(Vec<String>),
}

impl SettingValue {
    /// The declared type of this value.
    pub fn setting_type(&self) -> SettingType {
        match self {
            SettingValue::Bool(_) => SettingType::Bool,
            SettingValue::Text(_) => SettingType::Text,
            SettingValue::Integer(_) => SettingType::Integer,
            SettingValue::Float(_) => SettingType::Float,
            SettingValue::Choice(_) => SettingType::Choice,
            SettingValue::Path(_) => SettingType::Path,
            SettingValue::List(_) => SettingType::List,
        }
    }

    /// Whether an optional-capable value (path/list) is set. Always true for
    /// the always-present types.
    pub fn is_set(&self) -> bool {
        match self {
            SettingValue::Path(p) => !p.is_empty(),
            SettingValue::List(l) => !l.is_empty(),
            _ => true,
        }
    }

    /// Neutral single-line display form for read-only contexts and logs-free
    /// summaries. Controls format values themselves.
    pub fn display(&self) -> String {
        match self {
            SettingValue::Bool(b) => if *b { "On" } else { "Off" }.to_string(),
            SettingValue::Text(t) => {
                if t.is_empty() {
                    "Not set".to_string()
                } else {
                    t.clone()
                }
            }
            SettingValue::Integer(n) => n.to_string(),
            SettingValue::Float(n) => format!("{n}"),
            SettingValue::Choice(c) => c.clone(),
            SettingValue::Path(p) => {
                if p.is_empty() {
                    "Not set".to_string()
                } else {
                    p.clone()
                }
            }
            SettingValue::List(l) => {
                if l.is_empty() {
                    "Not set".to_string()
                } else {
                    l.join(", ")
                }
            }
        }
    }

    /// Value equality that treats equal floats exactly (mock values never
    /// carry NaN) and is type-strict.
    pub fn equals(&self, other: &SettingValue) -> bool {
        self == other
    }
}

/// Validate `value` against `descriptor` constraints. Pure; unit tested.
pub fn validate(descriptor: &SettingDescriptor, value: &SettingValue) -> Result<(), LicomanError> {
    use crate::descriptor::Control;

    let reject = |technical: String| {
        Err(LicomanError::new(
            ErrorKind::Validation,
            validation_message(descriptor, value),
            technical,
        ))
    };

    if value.setting_type() != descriptor.setting_type {
        return reject(format!(
            "type mismatch: expected {}, got {}",
            descriptor.setting_type.as_str(),
            value.setting_type().as_str()
        ));
    }

    match (value, &descriptor.control) {
        (SettingValue::Choice(choice), control) => {
            let options = match control {
                Control::Choice { options } => options,
                _ => {
                    return reject(format!(
                        "control {:?} cannot hold a choice value",
                        descriptor.control
                    ));
                }
            };
            if !options.iter().any(|o| o == choice) {
                return reject(format!(
                    "choice {choice:?} is not one of the declared options"
                ));
            }
        }
        (SettingValue::Integer(n), _) => {
            if let Some(max) = descriptor.max
                && *n as f64 > max
            {
                return reject(format!("integer {n} above maximum {max}"));
            }
            if let Some(min) = descriptor.min
                && (*n as f64) < min
            {
                return reject(format!("integer {n} below minimum {min}"));
            }
        }
        (SettingValue::Float(n), _) => {
            if let Some(max) = descriptor.max
                && *n > max
            {
                return reject(format!("float {n} above maximum {max}"));
            }
            if let Some(min) = descriptor.min
                && *n < min
            {
                return reject(format!("float {n} below minimum {min}"));
            }
        }
        _ => {}
    }
    Ok(())
}

fn validation_message(descriptor: &SettingDescriptor, value: &SettingValue) -> String {
    match descriptor.setting_type {
        SettingType::Choice => format!(
            "\"{}\" is not a valid option for {}.",
            value.display(),
            descriptor.label
        ),
        SettingType::Integer | SettingType::Float => {
            let mut msg = format!("{} must be", descriptor.label);
            match (descriptor.min, descriptor.max) {
                (Some(min), Some(max)) => {
                    msg.push_str(&format!(" between {min} and {max}."));
                }
                (Some(min), None) => msg.push_str(&format!(" at least {min}.")),
                (None, Some(max)) => msg.push_str(&format!(" at most {max}.")),
                (None, None) => msg.push_str(" a number."),
            }
            msg
        }
        _ => format!(
            "{} has an unsupported value for this control.",
            descriptor.label
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{ApplyMode, Control, Scope, SettingDescriptor};

    fn descriptor(control: Control, setting_type: SettingType) -> SettingDescriptor {
        SettingDescriptor {
            id: "test.setting",
            scope: Scope::Shared,
            category: "Test",
            label: "Test setting",
            description: "desc",
            setting_type,
            control,
            apply_mode: ApplyMode::Staged,
            restart_target: None,
            permission_scope: None,
            note: None,
            min: None,
            max: None,
        }
    }

    #[test]
    fn type_must_match_descriptor() {
        let d = descriptor(Control::Text { placeholder: "" }, SettingType::Text);
        assert!(validate(&d, &SettingValue::Text("ok".into())).is_ok());
        assert!(validate(&d, &SettingValue::Integer(3)).is_err());
    }

    #[test]
    fn choice_must_be_a_declared_option() {
        let d = descriptor(
            Control::Choice {
                options: vec!["A".into(), "B".into()],
            },
            SettingType::Choice,
        );
        assert!(validate(&d, &SettingValue::Choice("A".into())).is_ok());
        let err = validate(&d, &SettingValue::Choice("Z".into())).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Validation);
        assert_eq!(
            err.user_message,
            "\"Z\" is not a valid option for Test setting."
        );
    }

    #[test]
    fn numbers_respect_bounds() {
        let d = descriptor(Control::Number { step: 1., unit: "" }, SettingType::Integer);
        let mut d = d;
        d.min = Some(0.);
        d.max = Some(64.);
        assert!(validate(&d, &SettingValue::Integer(64)).is_ok());
        let err = validate(&d, &SettingValue::Integer(65)).unwrap_err();
        assert!(err.user_message.contains("between 0 and 64"));
        assert!(validate(&d, &SettingValue::Integer(-1)).is_err());
    }

    #[test]
    fn optional_types_report_set_state() {
        assert!(!SettingValue::Path(String::new()).is_set());
        assert!(SettingValue::Path("/tmp".into()).is_set());
        assert!(!SettingValue::List(vec![]).is_set());
        assert!(SettingValue::Bool(false).is_set());
    }

    #[test]
    fn display_forms() {
        assert_eq!(SettingValue::Bool(true).display(), "On");
        assert_eq!(SettingValue::Text(String::new()).display(), "Not set");
        assert_eq!(SettingValue::Integer(5).display(), "5");
        assert_eq!(
            SettingValue::List(vec!["a".into(), "b".into()]).display(),
            "a, b"
        );
    }
}
