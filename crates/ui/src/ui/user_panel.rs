//! The account panel (Scope::User): the current user's picture, username,
//! full name and password, with own-user changes through AccountsService.
//!
//! - The full-name and picture flows work whenever the daemon is reachable;
//!   the password flow too (polkit's change-own-user-data authorizes an
//!   active session — the GUI never collects a password for authorization).
//! - Without the daemon everything degrades to read-only with an honest
//!   note; identity and picture still render from /etc/passwd and ~/.face.
//! - Input values sync from the account cache at creation and on
//!   `AccountChanged` — applied at the next render where window access is
//!   available (`set_value` needs a window), guarded so user edits are not
//!   clobbered mid-flow.

use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputContentType, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::theme::Theme;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, Size, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, ClickEvent, Context, Entity, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, StatefulInteractiveElement as _, Styled, Subscription,
    Window, div, px,
};
use licoman_core::descriptor::Scope;

use crate::app_state::{AccountAction, AccountOp, AppEvent, AppState};
use crate::theme::metrics;
use crate::ui::window_heading;

pub struct UserAccountPanel {
    app: Entity<AppState>,
    name_input: Entity<InputState>,
    password_input: Entity<InputState>,
    verify_input: Entity<InputState>,
    /// Set by the AccountChanged subscription; consumed at the next render
    /// where `set_value` (which needs a window) can run.
    pending_sync: bool,
    /// The full name the inputs were last synced from (edit clobber guard).
    synced_name: Option<String>,
    _subscription: Subscription,
}

impl UserAccountPanel {
    pub fn new(app: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("Full name"));
        let password_input = cx.new(|cx| InputState::new(window, cx).placeholder("New password"));
        let verify_input = cx.new(|cx| InputState::new(window, cx).placeholder("Repeat password"));
        // Password fields start masked; the Input's mask_toggle button can
        // reveal them (the reveal keeps the value out of accessibility).
        for input in [&password_input, &verify_input] {
            input.update(cx, |state, cx| {
                state.set_masked(true, window, cx);
            });
        }
        let subscription = cx.subscribe(&app, |this, _app, event: &AppEvent, cx| {
            if matches!(event, AppEvent::AccountChanged) {
                this.pending_sync = true;
                cx.notify();
            }
        });
        Self {
            app,
            name_input,
            password_input,
            verify_input,
            pending_sync: true,
            synced_name: None,
            _subscription: subscription,
        }
    }

    // ---- intents ---------------------------------------------------------

    fn apply_full_name(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let value = self.name_input.read(cx).value().to_string();
        let current = self
            .app
            .read(cx)
            .account()
            .info
            .as_ref()
            .map(|info| info.full_name.clone())
            .unwrap_or_default();
        if value.trim().is_empty() || value.trim() == current.trim() {
            return;
        }
        self.app.update(cx, |state, cx| {
            state.set_account_full_name(value, cx);
        });
    }

    fn apply_password(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let new = self.password_input.read(cx).value().to_string();
        let verify = self.verify_input.read(cx).value().to_string();
        if new.is_empty() || new != verify {
            return;
        }
        self.app.update(cx, |state, cx| {
            state.set_account_password(new, cx);
        });
    }

    fn change_picture(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.app.read(cx).account().op_busied() {
            return; // one change at a time; buttons stay honest
        }
        cx.spawn(async move |this, cx| {
            // The XDG file-chooser portal round-trip: picker dialog, then
            // the Response signal — send().await resolves with both done.
            // Closing the dialog without a pick is a silent no-op.
            let picked = pick_picture_file().await;
            match picked {
                Ok(Some(path)) => {
                    let _ = this.update(cx, |this, cx| {
                        this.app.update(cx, |state, cx| {
                            state.set_account_avatar(path, cx);
                        })
                    });
                }
                Ok(None) => {}
                Err(message) => {
                    let _ = this.update(cx, |this, cx| {
                        this.app
                            .update(cx, |state, cx| state.fail_account_picture(message, cx))
                    });
                }
            }
        })
        .detach();
    }

    fn remove_picture(
        &mut self,
        _event: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.app.update(cx, |state, cx| {
            state.remove_account_avatar(cx);
        });
    }

    // ---- render helpers -----------------------------------------------------

    fn sync_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.pending_sync {
            return;
        }
        self.pending_sync = false;
        let (current_name, op) = {
            let state = self.app.read(cx);
            let name = state
                .account()
                .info
                .as_ref()
                .map(|info| info.full_name.clone())
                .unwrap_or_default();
            (name, state.account().op.clone())
        };
        // The name input tracks the authoritative value when it was not
        // touched since the last sync (after a successful change this
        // normalizes trimming too).
        if self.synced_name.as_deref() != Some(current_name.as_str())
            || self.name_input.read(cx).value().is_empty()
        {
            self.synced_name = Some(current_name.clone());
            if self.name_input.read(cx).value() != current_name.as_str() {
                self.name_input.update(cx, |state, cx| {
                    state.set_value(current_name.clone(), window, cx);
                });
            }
        }
        // A finished password change clears both fields.
        if op == AccountOp::Saved(AccountAction::Password) {
            for input in [&self.password_input, &self.verify_input] {
                if !input.read(cx).value().is_empty() {
                    input.update(cx, |state, cx| {
                        state.set_value(String::new(), window, cx);
                    });
                }
            }
        }
    }

    fn render_profile_card(&self, theme: &Theme, cx: &Context<Self>) -> AnyElement {
        let account = self.app.read(cx).account();
        let username = account
            .info
            .as_ref()
            .map(|info| format!("@{}", info.username))
            .unwrap_or_else(|| "…".to_string());
        let display_name = account
            .info
            .as_ref()
            .map(|info| {
                if info.full_name.is_empty() {
                    info.username.clone()
                } else {
                    info.full_name.clone()
                }
            })
            .unwrap_or_else(|| "Local account".to_string());

        let mut avatar = gpui_kit::component::avatar::Avatar::new().name(display_name.clone());
        if let Some(path) = &account.avatar {
            avatar = avatar.src(path.clone());
        }

        // The avatar itself is the pick control: clicking it opens the
        // portal file chooser, and hovering shows a scrim with a camera
        // hint (`group`/`group_hover`). The overlay lives inside the
        // container, so its clicks bubble to the same handler.
        let avatar_picker = h_flex()
            .id("avatar-pick")
            .group("avatar-pick")
            .relative()
            .cursor_pointer()
            .child(avatar.with_size(Size::Size(px(80.))))
            .child(
                h_flex()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .justify_center()
                    .items_center()
                    .gap_1()
                    .rounded_full()
                    .bg(gpui_kit::black().opacity(0.55))
                    .text_color(gpui_kit::component::white())
                    .opacity(0.0)
                    .group_hover("avatar-pick", |style| style.opacity(1.0))
                    .child(
                        Icon::new(gpui_kit::assets::IconName::Camera)
                            .with_size(Size::Size(px(18.))),
                    )
                    .child(div().text_size(px(metrics::TEXT_META)).child("Change")),
            )
            .on_click(cx.listener(Self::change_picture));

        h_flex()
            .gap_4()
            .items_center()
            .child(avatar_picker)
            .child(
                v_flex()
                    .min_w_0()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(metrics::TEXT_PAGE_HEADING))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .child(display_name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(metrics::TEXT_BODY))
                            .text_color(theme.muted_foreground)
                            .child(username.clone()),
                    )
                    .child(
                        Button::new("remove-picture")
                            .label("Remove picture")
                            .compact()
                            .disabled(account.avatar.is_none() || account.op_busied())
                            .on_click(cx.listener(Self::remove_picture)),
                    ),
            )
            .into_any_element()
    }
}

/// Open the portal file chooser and resolve the picked file to a filesystem
/// path. Closing the dialog without a pick (or portal absence in general
/// handled by the caller) yields `Ok(None)` — a silent no-op, never an
/// error text.
async fn pick_picture_file() -> Result<Option<std::path::PathBuf>, String> {
    use ashpd::desktop::file_chooser::{FileFilter, OpenFileRequest};
    let filter = FileFilter::new("Images")
        .mimetype("image/png")
        .mimetype("image/jpeg")
        .mimetype("image/webp")
        .mimetype("image/gif")
        .mimetype("image/svg+xml");
    let request = OpenFileRequest::default()
        .title("Choose a picture")
        .accept_label("Set picture")
        .filter(filter);
    let response = request
        .send()
        .await
        .map_err(|error| format!("The file chooser is unavailable: {error}"))?
        .response()
        .map_err(|error| match error {
            // The user closed the dialog — a silent no-op, never an error.
            ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled) => None,
            _ => Some(format!("The file chooser failed: {error}")),
        });
    match response {
        Ok(response) => {
            let Some(uri) = response.uris().first().map(|uri| uri.as_str()) else {
                return Ok(None);
            };
            let path = uri.strip_prefix("file://").unwrap_or(uri);
            Ok(Some(std::path::PathBuf::from(percent_decode(path))))
        }
        // Cancelled: the user closed the dialog — nothing happened.
        Err(None) => Ok(None),
        Err(Some(message)) => Err(message),
    }
}

/// Minimal `file://` percent-decoding (no new dependencies).
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(
                std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or(""),
                16,
            )
        {
            out.push(byte);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl Render for UserAccountPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_inputs(window, cx);
        let theme = cx.theme().clone();
        let account = self.app.read(cx).account().clone();
        let service = account.service;
        let daemon_ready = service == Some(true);
        let (new_password, verify_password) = {
            (
                self.password_input.read(cx).value().to_string(),
                self.verify_input.read(cx).value().to_string(),
            )
        };
        let passwords_match = new_password == verify_password;
        let name_value = self.name_input.read(cx).value().to_string();
        let current_name = account
            .info
            .as_ref()
            .map(|info| info.full_name.trim().to_string())
            .unwrap_or_default();
        let name_changed = !name_value.trim().is_empty() && name_value.trim() != current_name;

        let status_line: Option<AnyElement> = match &account.op {
            AccountOp::Idle => None,
            AccountOp::Busy(action) => Some(
                div()
                    .text_size(px(metrics::TEXT_META))
                    .text_color(theme.muted_foreground)
                    .child(match action {
                        AccountAction::FullName => "Saving full name…".to_string(),
                        AccountAction::Password => "Changing password…".to_string(),
                        AccountAction::Picture => "Setting picture…".to_string(),
                    })
                    .into_any_element(),
            ),
            AccountOp::Saved(action) => Some(
                div()
                    .text_size(px(metrics::TEXT_META))
                    .text_color(theme.foreground)
                    .child(match action {
                        AccountAction::FullName => "Full name saved.".to_string(),
                        AccountAction::Password => "Password changed.".to_string(),
                        AccountAction::Picture => "Picture updated.".to_string(),
                    })
                    .into_any_element(),
            ),
            AccountOp::Failed(_, message) => Some(
                div()
                    .text_size(px(metrics::TEXT_META))
                    .text_color(theme.danger)
                    .child(message.clone())
                    .into_any_element(),
            ),
        };

        let service_note = match service {
            None => Some("Checking for the accountsservice daemon…".to_string()),
            Some(false) => Some(
                "The accountsservice daemon is not reachable on the system \
                 bus, so changes are unavailable here. The identity and \
                 picture above are read from /etc/passwd and ~/.face."
                    .to_string(),
            ),
            Some(true) => None,
        };

        let password_mismatch_line = (!new_password.is_empty() && !passwords_match)
            .then(|| "The passwords do not match.".to_string());

        v_flex()
            .flex_1()
            .min_h_0()
            .id("user-account-content")
            .px(px(metrics::CONTENT_PADDING))
            .py_5()
            .gap_4()
            .overflow_y_scrollbar()
            .child(
                v_flex()
                    .max_w(px(metrics::CONTENT_MAX_WIDTH))
                    .gap_3()
                    .child(window_heading(Scope::User.label(), &theme))
                    .child(
                        div()
                            .text_size(px(metrics::TEXT_BODY))
                            .text_color(theme.muted_foreground)
                            .child(Scope::User.description()),
                    ),
            )
            .child(self.render_profile_card(&theme, cx))
            .when_some(status_line, |content, line| content.child(line))
            .when_some(service_note, |content, note| {
                content.child(
                    div()
                        .text_size(px(metrics::TEXT_META))
                        .text_color(theme.muted_foreground)
                        .child(note),
                )
            })
            .child(
                // Full name: one input + Apply.
                v_flex()
                    .max_w(px(metrics::CONTENT_MAX_WIDTH))
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(metrics::TEXT_LABEL))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.foreground)
                            .child("Full name"),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .max_w(px(metrics::CONTROL_COLUMN_WIDTH))
                                    .child(Input::new(&self.name_input).id("full-name-input")),
                            )
                            .child(
                                Button::new("apply-full-name")
                                    .label("Apply")
                                    .compact()
                                    .disabled(!name_changed || !daemon_ready || account.op_busied())
                                    .on_click(cx.listener(Self::apply_full_name)),
                            ),
                    ),
            )
            .child(
                // Password: new + verify + Apply; the service authorizes the
                // active session itself — no current-password prompt.
                v_flex()
                    .max_w(px(metrics::CONTENT_MAX_WIDTH))
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(metrics::TEXT_LABEL))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.foreground)
                            .child("Password"),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div().w(px(metrics::CONTROL_COLUMN_WIDTH)).child(
                                    Input::new(&self.password_input)
                                        .id("new-password-input")
                                        .mask_toggle()
                                        .content_type(InputContentType::NewPassword),
                                ),
                            )
                            .child(
                                div().w(px(metrics::CONTROL_COLUMN_WIDTH)).child(
                                    Input::new(&self.verify_input)
                                        .id("verify-password-input")
                                        .mask_toggle()
                                        .content_type(InputContentType::NewPassword),
                                ),
                            )
                            .child(
                                Button::new("apply-password")
                                    .label("Change password")
                                    .compact()
                                    .disabled(
                                        new_password.is_empty()
                                            || !passwords_match
                                            || !daemon_ready
                                            || account.op_busied(),
                                    )
                                    .on_click(cx.listener(Self::apply_password)),
                            ),
                    )
                    .when_some(password_mismatch_line, |content, note| {
                        content.child(
                            div()
                                .text_size(px(metrics::TEXT_META))
                                .text_color(theme.muted_foreground)
                                .child(note),
                        )
                    }),
            )
    }
}
