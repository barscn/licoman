//! The registry-driven control factory (04_MODULE_SCHEMA_AND_REGISTRY.md §6):
//! every registry-provided control is created here, once per window, from
//! the descriptor list, and rendered exclusively through the approved Quiet
//! Control components (Switch/Select/Slider/NumberInput/Input and the
//! read-only value). No window hard-codes a setting id any more — adding a
//! manifest module needs no GPUI source changes.
//!
//! The set owns its interactive entities (Select/Slider/Inputs keep local
//! display state) plus the subscriptions that turn their events into typed
//! [`Command::Edit`] dispatches, and re-syncs them from the store after
//! apply/discard so controls never present a stale value.

use std::collections::HashMap;

use gpui_kit::FontWeight;
use gpui_kit::component::Disableable as _;
use gpui_kit::component::IndexPath;
use gpui_kit::component::h_flex;
use gpui_kit::component::input::{Input, InputEvent, InputState, NumberInput};
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent, SelectState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState, SliderValue};
use gpui_kit::component::switch::Switch;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, IntoElement as _, ParentElement as _, Render,
    SharedString, Styled, Subscription, Window, div, px,
};
use licoman_core::descriptor::SettingDescriptor;
use licoman_core::descriptor::{ApplyMode, Control};
use licoman_core::state::SettingPhase;
use licoman_core::value::SettingType;
use licoman_core::value::SettingValue;
use licoman_core::{Command, SettingModel};

use crate::app_state::AppState;
use crate::theme::metrics;

/// One interactive control's entity, keyed by stable setting id.
#[derive(Clone)]
enum ControlEntity {
    Select(Entity<SelectState<SearchableVec<String>>>),
    Slider(Entity<SliderState>),
    Text(Entity<InputState>),
    Number(Entity<InputState>),
    /// Switch / ReadOnly / Action / Path display need no owned state.
    Stateless,
}

/// Owned controls for a set of descriptors.
pub struct ControlSet {
    entities: HashMap<String, ControlEntity>,
    _subscriptions: Vec<Subscription>,
}

impl ControlSet {
    /// Build entities and subscriptions for every editable control in
    /// `rows`. Call once per window; entities are re-synced with
    /// [`ControlSet::sync`].
    pub fn build(
        app: &Entity<AppState>,
        rows: &[(SettingDescriptor, SettingModel)],
        window: &mut Window,
        cx: &mut Context<impl Render + 'static>,
    ) -> Self {
        let mut entities: HashMap<String, ControlEntity> = HashMap::new();
        let mut subscriptions: Vec<Subscription> = Vec::new();

        for (descriptor, model) in rows {
            let id = descriptor.id.clone();
            if model.phase.blocks_editing() || descriptor.control == Control::ReadOnly {
                entities.insert(id, ControlEntity::Stateless);
                continue;
            }
            match &descriptor.control {
                Control::Choice { options } => {
                    let current = match model.effective() {
                        Some(SettingValue::Choice(choice)) => {
                            options.iter().position(|opt| opt == choice).unwrap_or(0)
                        }
                        _ => 0,
                    };
                    let state = cx.new(|cx| {
                        SelectState::new(
                            SearchableVec::new(options.clone()),
                            Some(IndexPath::new(current)),
                            window,
                            cx,
                        )
                    });
                    let app_for_select = app.clone();
                    let select_id = id.clone();
                    subscriptions.push(cx.subscribe(&state, {
                        move |_this, _select, event: &SelectEvent<SearchableVec<String>>, cx| {
                            if let SelectEvent::Confirm(Some(value)) = event {
                                let value = value.clone();
                                let select_id = select_id.clone();
                                app_for_select.update(cx, |state, cx| {
                                    state.dispatch(
                                        Command::Edit {
                                            id: select_id,
                                            value: SettingValue::Choice(value),
                                        },
                                        cx,
                                    );
                                });
                            }
                        }
                    }));
                    entities.insert(id, ControlEntity::Select(state));
                }
                Control::Slider { .. } => {
                    let (min, max, step, current) = slider_params(descriptor, model);
                    // Builder setters validate eagerly, so `max` must be
                    // raised before `min` (the default max is 100; a range
                    // like 1700–6500 K would panic mid-build otherwise).
                    let state = cx.new(|_| {
                        SliderState::new()
                            .max(max as f32)
                            .min(min as f32)
                            .step(step as f32)
                            .default_value(current as f32)
                    });
                    let app_for_slider = app.clone();
                    let slider_id = id.clone();
                    let setting_type = descriptor.setting_type;
                    subscriptions.push(cx.subscribe(&state, {
                        move |_this, _slider, event: &SliderEvent, cx| {
                            if let SliderEvent::Change(SliderValue::Single(v)) = event {
                                let value = match setting_type {
                                    SettingType::Integer => SettingValue::Integer(*v as i64),
                                    _ => SettingValue::Float(*v as f64),
                                };
                                let slider_id = slider_id.clone();
                                app_for_slider.update(cx, |state, cx| {
                                    state.dispatch(
                                        Command::Edit {
                                            id: slider_id,
                                            value,
                                        },
                                        cx,
                                    );
                                });
                            }
                        }
                    }));
                    entities.insert(id, ControlEntity::Slider(state));
                }
                Control::Number { .. } => {
                    let state = cx.new(|cx| {
                        let mut input = InputState::new(window, cx);
                        if let Some(current) = number_text(descriptor, model) {
                            input = input.default_value(&current);
                        }
                        if let Some(step) = number_step(descriptor) {
                            input = input.step(step);
                        }
                        if let Some(min) = descriptor.min {
                            input = input.min(min);
                        }
                        if let Some(max) = descriptor.max {
                            input = input.max(max);
                        }
                        input
                    });
                    let app_for_number = app.clone();
                    let number_id = id.clone();
                    let setting_type = descriptor.setting_type;
                    subscriptions.push(cx.subscribe(&state, {
                        move |_this, input, event: &InputEvent, cx| {
                            if matches!(event, InputEvent::Change) {
                                let text = input.read(cx).value().trim().to_string();
                                let value = parse_number(&text, setting_type);
                                if let Some(value) = value {
                                    let number_id = number_id.clone();
                                    app_for_number.update(cx, |state, cx| {
                                        state.dispatch(
                                            Command::Edit {
                                                id: number_id,
                                                value,
                                            },
                                            cx,
                                        );
                                    });
                                }
                            }
                        }
                    }));
                    entities.insert(id, ControlEntity::Number(state));
                }
                Control::Text { placeholder } => {
                    let state = cx.new(|cx| {
                        let mut input =
                            InputState::new(window, cx).placeholder(placeholder.clone());
                        if let Some(SettingValue::Text(text)) = model.effective() {
                            input = input.default_value(text.clone());
                        }
                        input
                    });
                    let app_for_text = app.clone();
                    let text_id = id.clone();
                    subscriptions.push(cx.subscribe(&state, {
                        move |_this, input, event: &InputEvent, cx| {
                            if matches!(event, InputEvent::Change) {
                                let value = input.read(cx).value().to_string();
                                let text_id = text_id.clone();
                                app_for_text.update(cx, |state, cx| {
                                    state.dispatch(
                                        Command::Edit {
                                            id: text_id,
                                            value: SettingValue::Text(value),
                                        },
                                        cx,
                                    );
                                });
                            }
                        }
                    }));
                    entities.insert(id, ControlEntity::Text(state));
                }
                Control::Path => {
                    let state = cx.new(|cx| {
                        let mut input = InputState::new(window, cx)
                            .placeholder("Path under your configuration root…");
                        if let Some(SettingValue::Path(text)) = model.effective() {
                            input = input.default_value(text.clone());
                        }
                        input
                    });
                    let app_for_path = app.clone();
                    let path_id = id.clone();
                    subscriptions.push(cx.subscribe(&state, {
                        move |_this, input, event: &InputEvent, cx| {
                            if matches!(event, InputEvent::Change) {
                                let value = input.read(cx).value().to_string();
                                let path_id = path_id.clone();
                                app_for_path.update(cx, |state, cx| {
                                    state.dispatch(
                                        Command::Edit {
                                            id: path_id,
                                            value: SettingValue::Path(value),
                                        },
                                        cx,
                                    );
                                });
                            }
                        }
                    }));
                    entities.insert(id, ControlEntity::Text(state));
                }
                Control::Switch | Control::List | Control::Action | Control::ReadOnly => {
                    entities.insert(id, ControlEntity::Stateless);
                }
            }
        }

        Self {
            entities,
            _subscriptions: subscriptions,
        }
    }

    /// Rebuild every entity from fresh rows. Needed when module
    /// applicability changed: rows that were unavailable (stateless) at
    /// build time become editable and need real entities.
    pub fn rebuild(
        &mut self,
        app: &Entity<AppState>,
        rows: &[(SettingDescriptor, SettingModel)],
        window: &mut Window,
        cx: &mut Context<impl Render + 'static>,
    ) {
        *self = Self::build(app, rows, window, cx);
    }

    /// Push store values back into the owned controls (after apply, discard
    /// or reload) so no control presents a stale value.
    pub fn sync(
        &mut self,
        rows: &[(SettingDescriptor, SettingModel)],
        window: &mut Window,
        cx: &mut Context<impl Render + 'static>,
    ) {
        for (descriptor, model) in rows {
            let Some(entity) = self.entities.get(&descriptor.id) else {
                continue;
            };
            match entity {
                ControlEntity::Select(state) => {
                    if let Some(SettingValue::Choice(choice)) = model.effective() {
                        state.update(cx, |s, cx| {
                            s.set_selected_value(choice, window, cx);
                        });
                    }
                }
                ControlEntity::Slider(state) => {
                    let (min, max, _step, current) = slider_params(descriptor, model);
                    let _ = (min, max);
                    state.update(cx, |s, cx| {
                        s.set_value(current as f32, window, cx);
                    });
                }
                ControlEntity::Text(state) => {
                    let text = match model.effective() {
                        Some(SettingValue::Text(t)) => t.clone(),
                        _ => String::new(),
                    };
                    if state.read(cx).value() != text.as_str() {
                        state.update(cx, |s, cx| {
                            s.set_value(text, window, cx);
                        });
                    }
                }
                ControlEntity::Number(state) => {
                    if let Some(text) = number_text(descriptor, model)
                        && state.read(cx).value() != text.as_str()
                    {
                        state.update(cx, |s, cx| {
                            s.set_value(text, window, cx);
                        });
                    }
                }
                ControlEntity::Stateless => {}
            }
        }
    }

    /// Render the control for one row. Stateless interactive controls
    /// (Switch) dispatch straight to the app entity; unavailable rows render
    /// through the read-only path.
    pub fn render(
        &self,
        descriptor: &SettingDescriptor,
        model: &SettingModel,
        app: &Entity<AppState>,
        theme: &gpui_kit::component::Theme,
    ) -> AnyElement {
        let unit = match &descriptor.control {
            Control::Slider { unit, .. } | Control::Number { unit, .. } => unit.clone(),
            _ => String::new(),
        };
        match self.entities.get(&descriptor.id) {
            Some(ControlEntity::Select(state)) => Select::new(state)
                .id(SharedString::from(descriptor.id.clone()))
                .menu_width(px(200.))
                .into_any_element(),
            Some(ControlEntity::Slider(state)) => {
                let value = match model.effective() {
                    Some(SettingValue::Float(n)) => *n,
                    Some(SettingValue::Integer(n)) => *n as f64,
                    _ => descriptor.min.unwrap_or(0.),
                };
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .child(Slider::new(state).flex_1())
                    .child(div_text(format!("{value}{unit}"), theme))
                    .into_any_element()
            }
            Some(ControlEntity::Number(state)) => {
                NumberInput::new(state).w(px(120.)).into_any_element()
            }
            Some(ControlEntity::Text(state)) => Input::new(state)
                .id(SharedString::from(descriptor.id.clone()))
                .w(px(220.))
                .into_any_element(),
            _ => {
                if descriptor.control == Control::Switch {
                    return switch_control(descriptor, model, app, theme);
                }
                read_only_for(descriptor, model, theme)
            }
        }
    }
}

/// The switch widget for boolean rows: dispatches a typed edit straight to
/// the store (disabled while the phase blocks editing).
fn switch_control(
    descriptor: &SettingDescriptor,
    model: &SettingModel,
    app: &Entity<AppState>,
    theme: &gpui_kit::component::Theme,
) -> AnyElement {
    let _ = theme;
    let checked = matches!(model.effective(), Some(SettingValue::Bool(true)));
    let editable = descriptor.apply_mode == ApplyMode::Staged && !model.phase.blocks_editing();
    let switch_id = SharedString::from(descriptor.id.clone());
    let setting_id = descriptor.id.clone();
    let app = app.clone();
    Switch::new(switch_id)
        .checked(checked)
        .disabled(!editable)
        .on_click(move |checked: &bool, _window, cx| {
            let value = SettingValue::Bool(*checked);
            let setting_id = setting_id.clone();
            app.update(cx, |state, cx| {
                state.dispatch(
                    Command::Edit {
                        id: setting_id,
                        value,
                    },
                    cx,
                );
            });
        })
        .into_any_element()
}

fn div_text(text: String, theme: &gpui_kit::component::Theme) -> gpui_kit::Div {
    div()
        .w(px(56.))
        .text_size(px(metrics::TEXT_BODY))
        .text_color(theme.foreground)
        .child(text)
}

fn slider_params(descriptor: &SettingDescriptor, model: &SettingModel) -> (f64, f64, f64, f64) {
    let (step, _unit) = match &descriptor.control {
        Control::Slider { step, unit } => (*step, unit.clone()),
        _ => (1., String::new()),
    };
    let min = descriptor.min.unwrap_or(0.);
    let max = descriptor.max.unwrap_or(100.);
    let current = match model.effective() {
        Some(SettingValue::Float(n)) => *n,
        Some(SettingValue::Integer(n)) => *n as f64,
        _ => min,
    };
    (min, max, step, current)
}

fn number_step(descriptor: &SettingDescriptor) -> Option<f64> {
    match &descriptor.control {
        Control::Number { step, .. } => Some(*step),
        _ => None,
    }
}

fn number_text(_descriptor: &SettingDescriptor, model: &SettingModel) -> Option<String> {
    match model.effective() {
        Some(SettingValue::Integer(n)) => Some(n.to_string()),
        Some(SettingValue::Float(n)) => Some(format!("{n}")),
        _ => None,
    }
}

fn parse_number(text: &str, setting_type: SettingType) -> Option<SettingValue> {
    match setting_type {
        SettingType::Integer => text.parse::<i64>().ok().map(SettingValue::Integer),
        SettingType::Float => text.parse::<f64>().ok().map(SettingValue::Float),
        _ => None,
    }
}

/// Read-only presentation for rows without an interactive entity: the
/// unavailable reason, the informational value, or the inert action
/// explanation — never a fake value.
fn read_only_for(
    descriptor: &SettingDescriptor,
    model: &SettingModel,
    theme: &gpui_kit::component::Theme,
) -> AnyElement {
    if model.phase == SettingPhase::Unavailable {
        let reason = model
            .error
            .as_ref()
            .map(|e| e.user_message.clone())
            .unwrap_or_else(|| "Unavailable".to_string());
        return div()
            .max_w(px(280.))
            .text_size(px(metrics::TEXT_META))
            .text_color(theme.muted_foreground)
            .child(reason)
            .into_any_element();
    }
    if descriptor.control == Control::Action {
        return div()
            .max_w(px(280.))
            .text_size(px(metrics::TEXT_META))
            .text_color(theme.muted_foreground)
            .child("Actions need a trusted backend (not available in this build).")
            .into_any_element();
    }
    let value = model
        .effective()
        .cloned()
        .unwrap_or(SettingValue::Text(String::new()));
    div()
        .text_size(px(metrics::TEXT_BODY))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.muted_foreground)
        .child(value.display())
        .into_any_element()
}

/// Compact truthful status line for detail panes that do not host the full
/// per-phase action buttons (the Applications window detail).
pub fn phase_status_text(model: &SettingModel) -> Option<String> {
    match model.phase {
        SettingPhase::Unknown => Some("Not read yet".to_string()),
        SettingPhase::Unavailable => Some(
            model
                .error
                .as_ref()
                .map(|e| e.user_message.clone())
                .unwrap_or_else(|| "Unavailable".to_string()),
        ),
        SettingPhase::Dirty => Some("Unsaved change".to_string()),
        SettingPhase::Validating => Some("Checking…".to_string()),
        SettingPhase::Pending => Some("Ready to apply".to_string()),
        SettingPhase::Applying => Some("Applying…".to_string()),
        SettingPhase::Applied => Some("Saved".to_string()),
        SettingPhase::Failed => Some(
            model
                .error
                .as_ref()
                .map(|e| format!("Failed — {}", e.user_message))
                .unwrap_or_else(|| "Failed".to_string()),
        ),
        SettingPhase::ExternalConflict => Some("Changed outside Licoman".to_string()),
        SettingPhase::Idle => None,
    }
}
