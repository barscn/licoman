//! Main window: scope sidebar, settings content, search palette, apply bar.
//! Structure follows 01_DESIGN_SYSTEM.md §4A. Since Phase 04 the content is
//! **registry-driven**: every row and control comes from validated module
//! manifests via the shared [`ControlSet`] factory — no setting id is
//! hard-coded here. Rendering reads owned snapshots from the core store;
//! every intent goes through `AppState::dispatch` with a typed command.

use crate::app_state::ScanState;
use gpui_kit::component::TitleBar;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::sidebar::{Sidebar, SidebarMenu, SidebarMenuItem};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::component::{Icon, IconName, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext, ClickEvent, Context, Entity, FontWeight, MouseButton,
    MouseDownEvent, ParentElement as _, Render, SharedString, Styled, Subscription, Window, div,
    px,
};
use gpui_kit::{
    Focusable as _, InteractiveElement as _, IntoElement as _, StatefulInteractiveElement as _,
};
use licoman_core::descriptor::{Scope, SettingDescriptor};
use licoman_core::error::ErrorKind;
use licoman_core::state::SettingPhase;
use licoman_core::{Command, SettingModel, WindowRole};

use crate::actions::*;
use crate::app_state::{AppEvent, AppState, open_or_focus};
use crate::theme::metrics;
use crate::ui::setting_controls::ControlSet;
use crate::ui::{OptionIntoAny as _, read_only_value, setting_row, window_heading};
use gpui_kit::Div;
use gpui_kit::component::theme::Theme;
use licoman_core::prefs::AppearanceMode;

/// Owned render snapshot for one setting row.
#[derive(Clone)]
struct Row {
    descriptor: SettingDescriptor,
    model: SettingModel,
}

/// One item of the virtualized settings list: a category header or a row
/// (Phase 08: the content list is a lazily-rendered `list`, so a catalog of
/// thousands of settings never builds element trees for off-screen rows).
#[derive(Clone)]
enum ListItem {
    Header { category: String, count: usize },
    Row(Box<Row>),
}

/// Category heading for the virtualized list: the same hairline style as
/// [`crate::ui::settings_group`]'s heading, with the row count on the right
/// (one compact line per category — never per-row badges).
fn group_heading_with_count(category: &str, count: usize, theme: &Theme) -> Div {
    h_flex()
        .justify_between()
        .items_baseline()
        .border_b_1()
        .border_color(theme.border)
        .pb_2()
        .child(
            div()
                .text_size(px(metrics::TEXT_GROUP_HEADING))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.foreground)
                .child(SharedString::from(category.to_string())),
        )
        .child(
            div()
                .text_size(px(metrics::TEXT_META))
                .text_color(theme.muted_foreground)
                .child(SharedString::from(format!("{count}"))),
        )
}

pub struct MainWindow {
    app: Entity<AppState>,
    header_search: Entity<InputState>,
    palette_input: Entity<InputState>,
    palette_open: bool,
    palette_selected: usize,
    /// Registry-driven controls for every editable setting in the sidebar
    /// scopes, keyed by stable setting id. Shared with the per-row views
    /// (the virtualized list renders each row through a
    /// [`SettingRowView`]); rebuilds swap the inner value wholesale.
    controls: std::rc::Rc<std::cell::RefCell<ControlSet>>,
    /// Set when the store changed; applied at the next render, where window
    /// access is available for control re-sync.
    resync_needed: bool,
    /// Set when the module applicability changed; the control set must be
    /// rebuilt (rows that were unavailable can become editable).
    rebuild_needed: bool,
    /// True while background rows are still unread; the transition to
    /// all-read triggers one control rebuild (rows become interactive when
    /// the read envelope flips to writable in fixture-write mode).
    had_pending_reads: bool,
    /// Set when the header search submits; applied at the next render, where
    /// window access is available.
    pending_palette_query: Option<String>,
    /// The flat (header|row) items the virtualized list renders, shared with
    /// the render closure (which runs later, during paint).
    list_items: std::rc::Rc<Vec<ListItem>>,
    /// One view per row item (parallel to `list_items`; `None` for header
    /// items). Built when the item set changes; each view reads fresh store
    /// state at draw time.
    row_views: std::rc::Rc<Vec<Option<Entity<SettingRowView>>>>,
    /// Fingerprint of the item set behind `list_items`; the list state is
    /// reset (dropping scroll position) only when this actually changes.
    list_items_signature: u64,
    /// Scroll/layout state of the virtualized list, kept across renders.
    list_state: gpui_kit::ListState,
    _subscriptions: Vec<Subscription>,
    _appearance_sub: Option<Subscription>,
}

impl MainWindow {
    pub fn new(app: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let header_search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search all settings… (Ctrl+K)"));
        let palette_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search all settings…"));

        // One control set over every row in the sidebar scopes (Phase 04:
        // the factory derives controls from descriptors, never from ids).
        let mut all_rows: Vec<(SettingDescriptor, SettingModel)> = Vec::new();
        for scope in Scope::SIDEBAR {
            all_rows.extend(app.read(cx).store.scope_rows(scope));
        }
        let controls = std::rc::Rc::new(std::cell::RefCell::new(ControlSet::build(
            &app, &all_rows, window, cx,
        )));

        let mut subscriptions = Vec::new();

        // The header search is the current-window entry into the global
        // search: pressing Enter opens the global palette with the query.
        // Window work happens in render (subscriptions have no window).
        subscriptions.push(cx.subscribe(&header_search, {
            move |this, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.pending_palette_query = Some(input.read(cx).value().to_string());
                    cx.notify();
                }
            }
        }));

        // Track app events: resync owned controls when the store changes
        // (discard, apply, module reload) so controls never present a stale
        // value.
        subscriptions.push(cx.subscribe(&app, {
            move |this, _app, event: &AppEvent, cx| match event {
                AppEvent::SettingsChanged => {
                    // The first read pass moves rows from Unknown to read
                    // (and, in fixture-write mode, to writable): the control
                    // set must be rebuilt then.
                    let pending = this.app.read(cx).store.background_rows_pending();
                    if this.had_pending_reads && !pending {
                        this.rebuild_needed = true;
                    }
                    this.had_pending_reads = pending;
                    this.resync_needed = true;
                    cx.notify();
                }
                AppEvent::ModulesChanged => {
                    this.rebuild_needed = true;
                    this.resync_needed = false;
                    cx.notify();
                }
                AppEvent::PreferencesChanged | AppEvent::EnvironmentChanged => cx.notify(),
            }
        }));

        // Palette Enter accepts the highlighted result.
        subscriptions.push(cx.subscribe(&palette_input, {
            move |this, _input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.accept_palette_result(cx);
                }
            }
        }));

        // Follow the desktop light/dark signal while `System` is selected.
        // The subscription must live as long as the window.
        let appearance_app = app.clone();
        let appearance_sub = window.observe_window_appearance(move |_window, cx| {
            appearance_app.update(cx, |state, cx| {
                if state.store.preferences().theme == AppearanceMode::System {
                    state.resolve_and_apply_theme(cx);
                }
            });
        });

        let mut this = Self {
            app,
            header_search,
            palette_input,
            palette_open: false,
            palette_selected: 0,
            controls,
            resync_needed: false,
            rebuild_needed: false,
            had_pending_reads: true,
            pending_palette_query: None,
            list_items: std::rc::Rc::new(Vec::new()),
            row_views: std::rc::Rc::new(Vec::new()),
            list_items_signature: 0,
            // Scroll state persists across renders; the overdraw keeps
            // scrolling smooth and the uniform hint pre-sizes the scrollbar
            // before every item has been measured.
            list_state: gpui_kit::ListState::new(0, gpui_kit::ListAlignment::Top, px(600.))
                .with_uniform_item_height(px(metrics::SETTING_ROW_MIN_HEIGHT)),
            _subscriptions: subscriptions,
            _appearance_sub: Some(appearance_sub),
        };
        this.sync_owned_controls(window, cx);
        this
    }

    /// Push store values into the controls that keep local display state
    /// (Select, Slider, Input) so the UI never shows a stale value after
    /// discard/apply/reload.
    fn sync_owned_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.resync_needed = false;
        let mut all_rows: Vec<(SettingDescriptor, SettingModel)> = Vec::new();
        for scope in Scope::SIDEBAR {
            all_rows.extend(self.app.read(cx).store.scope_rows(scope));
        }
        self.controls.borrow_mut().sync(&all_rows, window, cx);
    }

    /// Rebuild the control set after module applicability changed (rows
    /// that were unavailable at build time can now be editable).
    fn rebuild_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rebuild_needed = false;
        let mut all_rows: Vec<(SettingDescriptor, SettingModel)> = Vec::new();
        for scope in Scope::SIDEBAR {
            all_rows.extend(self.app.read(cx).store.scope_rows(scope));
        }
        self.controls
            .borrow_mut()
            .rebuild(&self.app, &all_rows, window, cx);
    }

    /// Visual-verification hook (`LICOMAN_DEMO=palette`): opens the palette
    /// without pointer/keyboard interaction.
    pub fn demo_open_palette(&mut self, cx: &mut Context<Self>) {
        self.palette_open = true;
        self.palette_selected = 0;
        cx.notify();
    }

    /// Demo hook (visual verification): open the Source details dialog for
    /// one setting id. Interactive clicks always did this through their
    /// listeners; a demo hook has no listener.
    pub fn demo_open_source_details(
        &mut self,
        setting_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let module_id = self
            .app
            .read(cx)
            .store
            .descriptor(setting_id)
            .map(|d| d.module_id.clone())
            .unwrap_or_default();
        let (lines, provenance) = source_details_lines(&self.app, setting_id, &module_id, cx);
        window.open_dialog(cx, move |dialog, _window, cx| {
            let theme = cx.theme().clone();
            let panel = details_panel(lines.clone(), provenance.clone(), &theme);
            dialog.title("Source details").keyboard(true).child(panel)
        });
        cx.notify();
    }

    fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = true;
        self.palette_selected = 0;
        let handle = self.palette_input.read(cx).focus_handle(cx).clone();
        window.focus(&handle, cx);
        cx.notify();
    }

    fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = false;
        // Return focus to the header search field (the invoking control).
        let handle = self.header_search.read(cx).focus_handle(cx).clone();
        window.focus(&handle, cx);
        cx.notify();
    }

    fn palette_results(&self, cx: &Context<Self>) -> Vec<licoman_core::search::SearchHit> {
        let query = self.palette_input.read(cx).value().to_string();
        self.app.read(cx).store.search(&query)
    }

    fn accept_palette_result(&mut self, cx: &mut Context<Self>) {
        let results = self.palette_results(cx);
        let picked = results
            .get(self.palette_selected.min(results.len().saturating_sub(1)))
            .cloned();
        if let Some(hit) = picked {
            if hit.scope == Scope::Application {
                // Application settings navigate into the Applications window
                // (00_START_HERE.md IA); selection is by stable identity.
                let module_id = hit.module_id.clone();
                self.app.update(cx, |state, cx| {
                    let identity = state.select_identity_for_module(&module_id);
                    state.dispatch(Command::SelectApp { id: identity }, cx);
                });
                self.open_applications_window(cx);
            } else {
                self.app.update(cx, |state, cx| {
                    state.dispatch(Command::SelectScope(hit.scope), cx);
                    state.push_history(
                        WindowRole::Main,
                        format!("scope:{}", hit.scope.label()),
                        cx,
                    );
                });
            }
        }
        self.palette_open = false;
        cx.notify();
    }

    // ---- row rendering ----------------------------------------------------

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let selected = self.app.read(cx).store.selected_scope();

        let mut menu = SidebarMenu::new().children([
            scope_item(Scope::Shared, IconName::Globe, selected, cx),
            scope_item(Scope::Desktop, IconName::LayoutDashboard, selected, cx),
            scope_item(Scope::Distribution, IconName::HardDrive, selected, cx),
        ]);

        // Applications is an action that opens/focuses the dedicated window.
        menu = menu.child(
            SidebarMenuItem::new("Applications")
                .icon(IconName::Inbox)
                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.open_applications_window(cx);
                })),
        );

        // Grounded environment summary (03_ENVIRONMENT_DISCOVERY.md UI
        // details): distro · desktop · protocol, each part omitted when
        // undetected, plus the live scan status. Partial results show as
        // they arrive — no pretending.
        let (summary, status_line) = {
            let state = self.app.read(cx);
            let summary = state
                .environment_snapshot()
                .map(|snapshot| snapshot.summary());
            let status_line = match state.scan_state() {
                ScanState::Loading { .. } => Some("Discovering environment…".to_string()),
                ScanState::Cancelled => Some("Scan cancelled — refreshing…".to_string()),
                ScanState::Idle => Some("Discovery pending…".to_string()),
                ScanState::Ready => None,
            };
            (summary, status_line)
        };

        // The sidebar footer's h_flex leaves children unconstrained; without
        // an explicit width a long summary renders as one clipped line.
        // Constrain to the sidebar content width (244 − 2×12 padding) so the
        // text wraps honestly instead of disappearing.
        let mut environment = v_flex()
            .w(px(metrics::SIDEBAR_WIDTH - 24.))
            .pb_3()
            .gap_1()
            .child(
                div()
                    .text_size(px(metrics::TEXT_META))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.muted_foreground)
                    .child("Environment"),
            )
            .child(
                div()
                    .text_size(px(metrics::TEXT_BODY))
                    .text_color(theme.foreground)
                    .child(summary.unwrap_or_else(|| "Discovering environment…".to_string())),
            );
        if let Some(status_line) = status_line {
            environment = environment.child(
                div()
                    .text_size(px(metrics::TEXT_META))
                    .text_color(theme.muted_foreground)
                    .child(status_line),
            );
        }

        Sidebar::new("scope-sidebar")
            .w(px(metrics::SIDEBAR_WIDTH))
            .collapsible(false)
            .child(menu)
            .footer(environment)
            .into_any_element()
    }

    fn render_content(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let (scope, category_filter, rows): (Scope, Option<String>, Vec<Row>) = {
            let state = self.app.read(cx);
            let scope = state.store.selected_scope();
            let category_filter = state.store.selected_category().map(str::to_string);
            let rows: Vec<Row> = state
                .store
                .scope_rows(scope)
                .into_iter()
                .map(|(descriptor, model)| Row { descriptor, model })
                .collect();
            (scope, category_filter, rows)
        };

        // The flat (header|row) item list, in category declaration order —
        // identical grouping to the previous eager rendering.
        let mut items: Vec<ListItem> = Vec::new();
        for category in self.app.read(cx).store.categories(scope) {
            // An explicit category selection narrows the content to that one
            // group (the palette and demo hooks dispatch it; scope switches
            // reset it).
            if let Some(filter) = &category_filter
                && category != *filter
            {
                continue;
            }
            let cat_rows: Vec<Row> = rows
                .iter()
                .filter(|row| row.descriptor.category == category)
                .cloned()
                .collect();
            items.push(ListItem::Header {
                category: category.clone(),
                count: cat_rows.len(),
            });
            items.extend(cat_rows.into_iter().map(|row| ListItem::Row(Box::new(row))));
        }

        // Scroll continuity: reset the list state (which also drops scroll
        // position) only when the item *set* changes — scope, filter, or the
        // registered setting ids. Value/state changes on existing rows
        // update in place.
        let mut signature_input = Vec::with_capacity(items.len() * 16);
        signature_input.extend_from_slice(format!("{scope:?}").as_bytes());
        if let Some(filter) = &category_filter {
            signature_input.extend_from_slice(filter.as_bytes());
        }
        for item in &items {
            match item {
                ListItem::Header { category, count } => {
                    signature_input.extend_from_slice(category.as_bytes());
                    signature_input.extend_from_slice(&count.to_le_bytes());
                }
                ListItem::Row(row) => {
                    signature_input.extend_from_slice(row.descriptor.id.as_bytes());
                }
            }
        }
        let signature = licoman_core::read::fnv1a(&signature_input);
        if signature != self.list_items_signature {
            self.list_state.reset(items.len());
            // One row view per row item, built once per item-set change.
            // Each view reads fresh store state at draw time, so value and
            // phase changes flow without rebuilding entities.
            let views: Vec<Option<Entity<SettingRowView>>> = items
                .iter()
                .map(|item| match item {
                    ListItem::Row(row) => Some(cx.new(|cx| {
                        SettingRowView::new(
                            self.app.clone(),
                            self.controls.clone(),
                            row.descriptor.clone(),
                            cx,
                        )
                    })),
                    ListItem::Header { .. } => None,
                })
                .collect();
            self.row_views = std::rc::Rc::new(views);
            self.list_items_signature = signature;
        }
        self.list_items = std::rc::Rc::new(items);

        // A category filter that matches nothing shows the empty state
        // too — never a blank pane.
        let empty_state = self.list_items.is_empty();
        let items_for_list = self.list_items.clone();
        let views_for_list = self.row_views.clone();
        let list_element = gpui_kit::list(self.list_state.clone(), move |ix, _window, cx| {
            let Some(item) = items_for_list.get(ix) else {
                return div().into_any_element();
            };
            match item {
                ListItem::Header { category, count } => {
                    let theme = cx.theme();
                    // Same visual as the old eager group heading, with the
                    // row count surfaced (cheap, header-only).
                    v_flex()
                        .pt_4()
                        .pb_2()
                        .child(group_heading_with_count(category, *count, theme))
                        .into_any_element()
                }
                // The row view entity was built when the item set changed;
                // drawing it reads the current store state. Handles are
                // cheap clones — only the visible range is ever drawn.
                ListItem::Row(_) => views_for_list
                    .get(ix)
                    .and_then(|view| view.clone())
                    .map(|view| view.into_any_element())
                    .unwrap_or_else(|| div().into_any_element()),
            }
        })
        .flex_1()
        .min_h_0();

        v_flex()
            .flex_1()
            .min_h_0()
            .id("settings-content")
            .px(px(metrics::CONTENT_PADDING))
            .py_5()
            .gap_4()
            .child(
                v_flex()
                    .max_w(px(metrics::CONTENT_MAX_WIDTH))
                    .gap_3()
                    .child(window_heading(scope.label(), &theme))
                    .child(
                        div()
                            .text_size(px(metrics::TEXT_BODY))
                            .text_color(theme.muted_foreground)
                            .child(scope.description()),
                    )
                    .when(!empty_state, |content| {
                        content.child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(
                                    gpui_kit::component::tag::Tag::secondary()
                                        .small()
                                        .child("Read-only reads"),
                                )
                                .child(
                                    div()
                                        .text_size(px(metrics::TEXT_META))
                                        .text_color(theme.muted_foreground)
                                        .child(
                                            "Rows come from built-in module manifests. Mock-backed \
                                             values are declared samples; real backends (GSettings, \
                                             config files, D-Bus services) are read from this \
                                             system and cannot be changed yet.",
                                        ),
                                ),
                        )
                    })
                    .when(empty_state, |content| {
                        // No providers registered (or none for this scope):
                        // a clear unavailable state, not a blank window.
                        content.child(
                            v_flex()
                                .gap_2()
                                .pt_6()
                                .child(
                                    div()
                                        .text_size(px(metrics::TEXT_LABEL))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(theme.foreground)
                                        .child("No settings available here"),
                                )
                                .child(
                                    div()
                                        .text_size(px(metrics::TEXT_BODY))
                                        .text_color(theme.muted_foreground)
                                        .child(
                                            "Licoman found no settings providers for this scope. Environments without supported providers stay usable and show this explanation instead of an empty pane.",
                                        ),
                                ),
                        )
                    }),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .max_w(px(metrics::CONTENT_MAX_WIDTH))
                    .child(list_element),
            )
            .into_any_element()
    }

    fn render_apply_bar(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme().clone();
        let (pending, applying, confirming) = {
            let store = &self.app.read(cx).store;
            (
                store.pending_count(),
                store.applying(),
                store.confirmation_pending().len(),
            )
        };
        if pending == 0 && !applying {
            return None;
        }
        let needs_confirmation = confirming > 0 && !applying;
        let bar_text = if applying {
            "Applying changes…".to_string()
        } else if needs_confirmation && confirming == pending {
            format!(
                "{confirming} change{} need{} your confirmation",
                if confirming == 1 { "" } else { "s" },
                if confirming == 1 { "s" } else { "" }
            )
        } else if needs_confirmation {
            format!(
                "{pending} change{} pending · {confirming} need confirmation",
                if pending == 1 { "" } else { "s" }
            )
        } else {
            format!(
                "{pending} change{} pending",
                if pending == 1 { "" } else { "s" }
            )
        };
        Some(
            h_flex()
                .justify_between()
                .items_center()
                .px(px(metrics::CONTENT_PADDING))
                .py_3()
                .border_t_1()
                .border_color(theme.border)
                .bg(theme.popover)
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Icon::new(IconName::CircleAlert)
                                .small()
                                .text_color(theme.warning),
                        )
                        .child(
                            div()
                                .text_size(px(metrics::TEXT_LABEL))
                                .text_color(theme.foreground)
                                .child(bar_text),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("discard-changes")
                                .label("Discard")
                                .ghost()
                                .disabled(applying)
                                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                                    this.app.update(cx, |state, cx| {
                                        state.dispatch(Command::DiscardAll, cx);
                                    });
                                })),
                        )
                        .when(needs_confirmation, |bar| {
                            bar.child(
                                Button::new("review-changes")
                                    .label("Review…")
                                    .primary()
                                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                        this.open_confirmation_dialog(window, cx);
                                    })),
                            )
                        })
                        .child(
                            Button::new("apply-changes")
                                .label(if applying { "Applying…" } else { "Apply" })
                                .primary()
                                .loading(applying)
                                .disabled(applying || needs_confirmation)
                                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                                    this.app.update(cx, |state, cx| {
                                        state.dispatch(Command::ApplyAll, cx);
                                    });
                                })),
                        ),
                ),
        )
        .into_any()
    }

    /// The confirmation dialog for staged changes whose core-derived policy
    /// is Confirmation (06_SAFE_WRITE_ENGINE.md §1: the policy is enforced
    /// by the core; this dialog is where the explicit confirmation happens).
    /// It shows effect, target provenance and restart behavior per row; the
    /// Apply button dispatches one `ConfirmApply` per listed row.
    fn open_confirmation_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let app = self.app.clone();
        let rows: Vec<(String, String, Option<String>)> = {
            let state = self.app.read(cx);
            state
                .store
                .confirmation_pending()
                .iter()
                .filter_map(|id| {
                    let descriptor = state.store.descriptor(id)?;
                    let target = descriptor
                        .restart_target
                        .clone()
                        .or_else(|| state.store.reset_support(id).map(|_| String::new()));
                    let target_note = state
                        .store
                        .model(id)
                        .and_then(|m| m.read_meta.as_ref())
                        .map(|meta| meta.details.target.clone());
                    Some((
                        descriptor.label.clone(),
                        target_note.unwrap_or_default(),
                        target,
                    ))
                })
                .collect()
        };
        if rows.is_empty() {
            return;
        }
        let count = rows.len();
        window.open_dialog(cx, move |dialog, _window, cx| {
            let theme = cx.theme().clone();
            let app = app.clone();
            let rows = rows.clone();
            dialog.title("Confirm changes").keyboard(true).child(
                v_flex()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(metrics::TEXT_META))
                            .text_color(theme.muted_foreground)
                            .child(
                                "These changes are high-impact. They run through the \
                                     safe write engine with a conflict check, a backup of \
                                     the previous value and a verified readback.",
                            ),
                    )
                    .children(rows.iter().map(|(label, target, restart)| {
                        v_flex()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_size(px(metrics::TEXT_LABEL))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(label.clone()),
                            )
                            .children((!target.is_empty()).then(|| {
                                div()
                                    .text_size(px(metrics::TEXT_META))
                                    .text_color(theme.muted_foreground)
                                    .child(target.clone())
                            }))
                            .children(restart.clone().map(|restart| {
                                div()
                                    .text_size(px(metrics::TEXT_META))
                                    .text_color(theme.warning)
                                    .child(format!("Applies after restarting {restart}"))
                            }))
                    }))
                    .child(
                        h_flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("confirm-cancel")
                                    .label("Cancel")
                                    .ghost()
                                    .on_click(|_, window, cx| {
                                        window.close_dialog(cx);
                                    }),
                            )
                            .child(
                                Button::new("confirm-apply")
                                    .label(format!("Apply {count}"))
                                    .primary()
                                    .on_click(move |_, window, cx| {
                                        window.close_dialog(cx);
                                        let app = app.clone();
                                        let ids = {
                                            let state = app.read(cx);
                                            state.store.confirmation_pending().to_vec()
                                        };
                                        app.update(cx, |state, cx| {
                                            for id in ids {
                                                state.dispatch(Command::ConfirmApply { id }, cx);
                                            }
                                        });
                                    }),
                            ),
                    ),
            )
        });
        cx.notify();
    }

    fn render_palette(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.palette_open {
            return None;
        }
        let theme = cx.theme().clone();
        let results = self.palette_results(cx);
        let count = results.len();
        let selected = self.palette_selected.min(count.saturating_sub(1));

        let items: Vec<AnyElement> = results
            .iter()
            .enumerate()
            .map(|(i, hit)| {
                let label = hit.label.clone();
                let breadcrumb = hit.breadcrumb.clone();
                // Apply status next to the breadcrumb (04 §6): the row's
                // current phase, looked up by stable id.
                let status = {
                    let store = &self.app.read(cx).store;
                    store
                        .model(&hit.id)
                        .and_then(crate::ui::setting_controls::phase_status_text)
                };
                let is_selected = i == selected;
                div()
                    .id(SharedString::from(format!("palette-result-{i}")))
                    .px_3()
                    .py_2()
                    .rounded_sm()
                    .when(is_selected, |row| row.bg(theme.selection))
                    .hover(|row| row.bg(theme.accent))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.palette_selected = i;
                        this.accept_palette_result(cx);
                    }))
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_size(px(metrics::TEXT_LABEL))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(theme.foreground)
                                            .child(label),
                                    )
                                    .children(status.map(|text| {
                                        gpui_kit::component::tag::Tag::warning().small().child(text)
                                    })),
                            )
                            .child(
                                div()
                                    .text_size(px(metrics::TEXT_META))
                                    .text_color(theme.muted_foreground)
                                    .child(breadcrumb),
                            ),
                    )
                    .into_any_element()
            })
            .collect();

        let query_is_empty = self.palette_input.read(cx).value().trim().is_empty();
        let empty = count == 0;
        Some(
            h_flex()
                .absolute()
                .inset_0()
                .bg(theme.overlay)
                .key_context("SearchPalette")
                .items_start()
                .justify_center()
                .pt(px(64.))
                .on_action(cx.listener(|this, _: &DismissPalette, window, cx| {
                    this.close_palette(window, cx);
                }))
                .on_action(cx.listener(move |this, _: &PaletteNext, _window, cx| {
                    if this.palette_selected + 1 < count {
                        this.palette_selected += 1;
                        cx.notify();
                    }
                }))
                .on_action(cx.listener(|this, _: &PalettePrev, _window, cx| {
                    this.palette_selected = this.palette_selected.saturating_sub(1);
                    cx.notify();
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        // Clicking the backdrop dismisses; clicks on the panel
                        // stop propagation before reaching the backdrop.
                        this.close_palette(window, cx);
                    }),
                )
                .child(
                    v_flex()
                        .w(px(560.))
                        .max_h(px(420.))
                        .bg(theme.popover)
                        .border_1()
                        .border_color(theme.border)
                        .rounded(px(metrics::RADIUS_LG))
                        .overflow_hidden()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .child(
                            div()
                                .p_3()
                                .border_b_1()
                                .border_color(theme.border)
                                .child(
                                    Input::new(&self.palette_input)
                                        .id("palette-input")
                                        .appearance(false)
                                        .large(),
                                ),
                        )
                        .when(empty, |panel| {
                            panel.child(
                                v_flex()
                                    .p_6()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_size(px(metrics::TEXT_LABEL))
                                            .text_color(theme.foreground)
                                            .child(if query_is_empty {
                                                "Search all settings"
                                            } else {
                                                "No matching settings"
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(metrics::TEXT_BODY))
                                            .text_color(theme.muted_foreground)
                                            .child(if query_is_empty {
                                                "Type to search across every registered setting; results show scope → category → setting."
                                            } else {
                                                "Try a different search. Built-in modules cover Shared, Desktop, Distribution and Applications."
                                            }),
                                    ),
                            )
                        })
                        .when(!empty, |panel| panel.children(items)),
                ),
        )
        .into_any()
    }

    fn open_applications_window(&mut self, cx: &mut Context<Self>) {
        let app = self.app.clone();
        open_or_focus(
            WindowRole::Applications,
            move |cx| crate::windows::open_apps_window(&app, cx),
            cx,
        );
    }

    fn open_preferences_window(&mut self, cx: &mut Context<Self>) {
        let app = self.app.clone();
        open_or_focus(
            WindowRole::Preferences,
            move |cx| crate::windows::open_preferences_window(&app, cx),
            cx,
        );
    }
}

fn model_restart_target(model: &SettingModel) -> Option<String> {
    if model.restart_required {
        Some("the affected program".to_string())
    } else {
        None
    }
}

/// Collect the Source details data for one setting: exact backend type,
/// schema/key or path, value origin, freshness and diagnostics, plus the
/// module's recorded provenance. By construction this carries metadata
/// only — never configuration contents (05_READ_ONLY_BACKENDS.md §4).
fn source_details_lines(
    app: &Entity<AppState>,
    setting_id: &str,
    module_id: &str,
    cx: &App,
) -> (Vec<(String, String)>, Vec<String>) {
    let state = app.read(cx);
    let model = state.store.model(setting_id);
    let provenance: Vec<String> = state
        .module_provider()
        .with_registry(|registry| registry.provenance_of(module_id));

    let mut lines: Vec<(String, String)> = Vec::new();
    if let Some(meta) = model.and_then(|m| m.read_meta.as_ref()) {
        lines.push(("Backend".into(), meta.details.backend_kind.to_string()));
        lines.push(("Source".into(), meta.details.target.clone()));
        let mut origin = meta.origin.label().to_string();
        if let Some(note) = &meta.details.origin_note {
            origin.push_str(&format!(" — {note}"));
        }
        lines.push(("Value origin".into(), origin));
        if meta.details.imported {
            lines.push((
                "Imported".into(),
                "yes (the value comes from an imported file)".into(),
            ));
        }
        let age = meta
            .freshness
            .read_at
            .elapsed()
            .map(|d| format!("{} s ago", d.as_secs()))
            .unwrap_or_else(|_| "just now".to_string());
        lines.push((
            "Freshness".into(),
            format!("read {age} · fingerprint {}", meta.freshness.fingerprint),
        ));
        match &meta.writable {
            licoman_core::read::Writable::Yes => {
                lines.push(("Writable".into(), "yes (mock source)".into()));
            }
            licoman_core::read::Writable::No { reason } => {
                lines.push(("Writable".into(), reason.clone()));
            }
            licoman_core::read::Writable::NeedsPermission { auth } => {
                lines.push(("Writable".into(), format!("requires authorization: {auth}")));
            }
            licoman_core::read::Writable::Unknown => {}
        }
        for diagnostic in &meta.details.diagnostics {
            lines.push(("Diagnostic".into(), diagnostic.clone()));
        }
    } else if let Some(error) = model.and_then(|m| m.error.as_ref()) {
        lines.push(("State".into(), "unavailable".into()));
        lines.push(("Reason".into(), error.user_message.clone()));
        lines.push(("Technical".into(), error.technical.clone()));
    }
    (lines, provenance)
}

/// Render the Source details panel from its (clonable) data.
fn details_panel(
    lines: Vec<(String, String)>,
    provenance: Vec<String>,
    theme: &gpui_kit::component::Theme,
) -> AnyElement {
    let mut panel = v_flex().gap_2();
    for (label, value) in lines {
        panel = panel.child(
            v_flex()
                .gap_0p5()
                .child(
                    div()
                        .text_size(px(metrics::TEXT_META))
                        .text_color(theme.muted_foreground)
                        .child(label),
                )
                .child(div().text_size(px(metrics::TEXT_BODY)).child(value)),
        );
    }
    if !provenance.is_empty() {
        let mut prov = v_flex().gap_0p5();
        for line in provenance {
            prov = prov.child(
                div()
                    .text_size(px(metrics::TEXT_META))
                    .text_color(theme.muted_foreground)
                    .child(line),
            );
        }
        panel = panel.child(
            v_flex()
                .gap_0p5()
                .child(
                    div()
                        .text_size(px(metrics::TEXT_META))
                        .text_color(theme.muted_foreground)
                        .child("Module provenance"),
                )
                .child(prov),
        );
    }
    panel.into_any_element()
}

fn scope_item(
    scope: Scope,
    icon: IconName,
    selected: Scope,
    cx: &mut Context<MainWindow>,
) -> SidebarMenuItem {
    let active = scope == selected;
    SidebarMenuItem::new(scope.label())
        .icon(icon)
        .active(active)
        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
            this.app.update(cx, |state, cx| {
                state.dispatch(Command::SelectScope(scope), cx);
                state.push_history(WindowRole::Main, format!("scope:{}", scope.label()), cx);
            });
        }))
}

/// One setting row as its own view entity (Phase 08): the virtualized
/// content list builds one of these per row item when the item set changes
/// and the list draws only the visible range. State is read fresh from the
/// store at draw time, so applies, conflicts and read passes flow through
/// without rebuilding entities. Listeners only touch the shared `app`
/// entity — exactly what the previous `Context<MainWindow>` listeners did.
struct SettingRowView {
    app: Entity<AppState>,
    controls: std::rc::Rc<std::cell::RefCell<ControlSet>>,
    descriptor: SettingDescriptor,
}

impl SettingRowView {
    fn new(
        app: Entity<AppState>,
        controls: std::rc::Rc<std::cell::RefCell<ControlSet>>,
        descriptor: SettingDescriptor,
        cx: &mut Context<Self>,
    ) -> Self {
        // Re-render when store state changes (the store notifies on every
        // AppEvent; drawing reads fresh state). Without this the framework's
        // render caching would keep stale rows on screen.
        let mut subscriptions = Vec::new();
        subscriptions.push(cx.subscribe(&app, |_, _, _: &AppEvent, cx| {
            cx.notify();
        }));
        let _ = subscriptions; // dropped with the view (fine: notify-only)
        Self {
            app,
            controls,
            descriptor,
        }
    }

    /// The full row: label/description left, control right, status lines
    /// below (moved verbatim from the eager renderer, same visuals).
    fn render_setting(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let model = self.app.read(cx).store.model(&self.descriptor.id).cloned();
        let Some(model) = model else {
            return div().into_any_element();
        };
        let row = Row {
            descriptor: self.descriptor.clone(),
            model,
        };
        self.render_row(&row, cx)
    }

    fn render_row(&mut self, row: &Row, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let control = self.render_control(row, cx);
        let status = self.render_status(row, cx);
        setting_row(
            SharedString::from(format!("setting-row-{}", row.descriptor.id)),
            row.descriptor.label.clone(),
            row.descriptor.description.clone(),
            control,
            status,
            &theme,
        )
        .into_any_element()
    }

    fn render_control(&mut self, row: &Row, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let unavailable_display = match &row.model.error {
            Some(error) if error.kind == ErrorKind::Permission => "Not available",
            _ => "Unavailable",
        };

        // Unavailable rows (cannot be read here) never show a fake value.
        if row.model.phase == SettingPhase::Unavailable {
            return read_only_value(unavailable_display, &theme).into_any_element();
        }

        self.controls
            .borrow()
            .render(&row.descriptor, &row.model, &self.app, &theme)
    }

    fn render_status(&mut self, row: &Row, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme().clone();
        let id = row.descriptor.id.clone();
        match &row.model.phase {
            SettingPhase::Unknown => Some(
                div()
                    .text_color(theme.muted_foreground)
                    .child("Not read yet"),
            )
            .into_any(),
            SettingPhase::Unavailable => {
                let error = row.model.error.as_ref()?;
                if error.kind == ErrorKind::Permission {
                    let authorize_id = SharedString::from(format!("authorize-{id}"));
                    Some(
                        v_flex()
                            .gap_1()
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .text_color(theme.muted_foreground)
                                    .child(
                                        Icon::new(gpui_kit::assets::IconName::Lock).small(),
                                    )
                                    .child(error.user_message.clone()),
                            )
                            .child(
                                h_flex().gap_2().child(
                                    Button::new(authorize_id)
                                        .label("Authorize…")
                                        .outline()
                                        .xsmall()
                                        .on_click(cx.listener(
                                            |_this, _: &ClickEvent, window, cx| {
                                                window.open_alert_dialog(cx, |alert, _window, _cx| {
                                                    alert
                                                        .title(
                                                            "Authorization is not available in this build",
                                                        )
                                                        .description(
                                                            "Licoman runs unprivileged. Privileged edits are deferred until the safe write engine and its authorization flow are implemented and reviewed (Phases 06–07). No system configuration was modified.",
                                                        )
                                                });
                                            },
                                        )),
                                ),
                            ),
                    )
                    .into_any()
                } else {
                    Some(
                        div()
                            .text_color(theme.muted_foreground)
                            .child(error.user_message.clone()),
                    )
                    .into_any()
                }
            }
            SettingPhase::PermissionRequired => {
                let scope = row
                    .descriptor
                    .permission_scope
                    .clone()
                    .or_else(|| {
                        row.model
                            .error
                            .as_ref()
                            .map(|_| "an authorization boundary".to_string())
                    })
                    .unwrap_or_else(|| "an authorization boundary".to_string());
                let authorize_id = SharedString::from(format!("authorize-{id}"));
                let revert_setting_id = id.clone();
                Some(
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_1()
                                .items_center()
                                .text_color(theme.muted_foreground)
                                .child(Icon::new(gpui_kit::assets::IconName::Lock).small())
                                .child(format!("Permission required — {scope}")),
                        )
                        .child(
                            h_flex().gap_2().child(
                                Button::new(authorize_id)
                                    .label("Authorize…")
                                    .outline()
                                    .xsmall()
                                    .on_click(cx.listener(
                                        |_this, _: &ClickEvent, window, cx| {
                                            window.open_alert_dialog(cx, |alert, _window, _cx| {
                                                alert
                                                    .title(
                                                        "Authorization is not available in this build",
                                                    )
                                                    .description(
                                                        "Licoman runs unprivileged. Privileged writes are denied by default by the permission broker; no password is ever requested and no system configuration was modified.",
                                                    )
                                            });
                                        },
                                    )),
                            ),
                        )
                        .child(self.revert_edit_button(&revert_setting_id, cx))
                            .into_any_element(),
                )
            }
            SettingPhase::Idle => {
                // Static provider notes (why a row is read-only) and known
                // restart requirements are truthful status lines.
                let reset_line = self.reset_action_line(row, cx);
                if let Some(note) = &row.descriptor.note {
                    return Some(
                        v_flex()
                            .gap_1()
                            .child(div().text_color(theme.muted_foreground).child(note.clone()))
                            .children(reset_line)
                            .children(self.source_details_line(row, &theme, cx))
                            .into_any_element(),
                    );
                }
                if let Some(target) = row
                    .descriptor
                    .restart_target
                    .clone()
                    .or_else(|| model_restart_target(&row.model))
                {
                    return Some(
                        v_flex()
                            .gap_1()
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .text_color(theme.warning)
                                    .child(Icon::new(IconName::TriangleAlert).small())
                                    .child(format!("Applies after restarting {target}")),
                            )
                            .children(reset_line)
                            .children(self.source_details_line(row, &theme, cx))
                            .into_any_element(),
                    );
                }
                Some(
                    v_flex()
                        .gap_1()
                        .children(reset_line)
                        .children(self.source_details_line(row, &theme, cx))
                        .into_any_element(),
                )
            }
            SettingPhase::Dirty => Some(
                h_flex()
                    .gap_1()
                    .items_center()
                    .text_color(theme.warning)
                    .child(Icon::new(IconName::CircleAlert).small())
                    .child("Unsaved change"),
            )
            .into_any(),
            SettingPhase::Validating => {
                Some(div().text_color(theme.muted_foreground).child("Checking…")).into_any()
            }
            SettingPhase::Pending => Some(
                div()
                    .text_color(theme.muted_foreground)
                    .child("Ready to apply"),
            )
            .into_any(),
            SettingPhase::Applying => Some(
                h_flex()
                    .gap_1()
                    .items_center()
                    .text_color(theme.muted_foreground)
                    .child("Applying…"),
            )
            .into_any(),
            SettingPhase::Applied => {
                let undo_id = SharedString::from(format!("undo-{id}"));
                let undo_setting_id = id.clone();
                let undo_available = self.app.read(cx).store.undo_available(&id);
                let persistence_note = match row.model.persistence {
                    Some(licoman_core::Persistence::RuntimeOnly) => {
                        " — runtime only, a restart of the service reverts it"
                    }
                    _ => "",
                };
                let saved_text = if row.model.restart_required {
                    format!(
                        "Saved — applies after restarting {}{persistence_note}",
                        row.descriptor
                            .restart_target
                            .clone()
                            .unwrap_or_else(|| "the affected program".to_string())
                    )
                } else {
                    format!("Saved{persistence_note}")
                };
                Some(
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_1()
                                .items_center()
                                .text_color(theme.success)
                                .child(Icon::new(IconName::CircleCheck).small())
                                .child(saved_text),
                        )
                        .when(undo_available, |line| {
                            line.child(
                                h_flex().gap_2().child(
                                    Button::new(undo_id)
                                        .label("Undo")
                                        .ghost()
                                        .xsmall()
                                        .on_click(cx.listener(
                                            move |this, _: &ClickEvent, _window, cx| {
                                                let setting_id = undo_setting_id.clone();
                                                this.app.update(cx, |state, cx| {
                                                    state.dispatch(
                                                        Command::Undo { id: setting_id },
                                                        cx,
                                                    );
                                                });
                                            },
                                        )),
                                ),
                            )
                        }),
                )
                .into_any()
            }
            SettingPhase::Failed => {
                let error = row.model.error.as_ref()?;
                let retry_id = SharedString::from(format!("retry-{id}"));
                let revert_id = SharedString::from(format!("revert-{id}"));
                let retry_setting_id = id.clone();
                let revert_setting_id = id.clone();
                Some(
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_1()
                                .items_center()
                                .text_color(theme.danger)
                                .child(Icon::new(IconName::CircleX).small())
                                .child(format!("Failed — {}", error.user_message)),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Button::new(retry_id)
                                        .label("Retry")
                                        .outline()
                                        .xsmall()
                                        .on_click(cx.listener(
                                            move |this, _: &ClickEvent, _window, cx| {
                                                let setting_id = retry_setting_id.clone();
                                                this.app.update(cx, |state, cx| {
                                                    state.dispatch(
                                                        Command::Retry { id: setting_id },
                                                        cx,
                                                    );
                                                });
                                            },
                                        )),
                                )
                                .child(
                                    Button::new(revert_id)
                                        .label("Revert")
                                        .ghost()
                                        .xsmall()
                                        .on_click(cx.listener(
                                            move |this, _: &ClickEvent, _window, cx| {
                                                let setting_id = revert_setting_id.clone();
                                                this.app.update(cx, |state, cx| {
                                                    state.dispatch(
                                                        Command::Revert { id: setting_id },
                                                        cx,
                                                    );
                                                });
                                            },
                                        )),
                                ),
                        ),
                )
                .into_any()
            }
            SettingPhase::ExternalConflict => {
                let reload_id = SharedString::from(format!("reload-{id}"));
                let compare_id = SharedString::from(format!("compare-{id}"));
                let reload_setting_id = id.clone();
                let stored = row
                    .model
                    .stored
                    .as_ref()
                    .map(|v| v.display())
                    .unwrap_or_default();
                let external = row
                    .model
                    .external
                    .as_ref()
                    .map(|v| v.display())
                    .unwrap_or_default();
                Some(
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_1()
                                .items_center()
                                .text_color(theme.warning)
                                .child(Icon::new(IconName::RefreshCw).small())
                                .child("Changed outside Licoman"),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Button::new(reload_id)
                                        .label("Reload")
                                        .outline()
                                        .xsmall()
                                        .on_click(cx.listener(
                                            move |this, _: &ClickEvent, _window, cx| {
                                                let setting_id = reload_setting_id.clone();
                                                this.app.update(cx, |state, cx| {
                                                    state.dispatch(
                                                        Command::Reload { id: setting_id },
                                                        cx,
                                                    );
                                                });
                                            },
                                        )),
                                )
                                .child(
                                    Button::new(compare_id)
                                        .label("Compare")
                                        .ghost()
                                        .xsmall()
                                        .on_click(cx.listener(
                                            move |_this, _: &ClickEvent, window, cx| {
                                                let stored = stored.clone();
                                                let external = external.clone();
                                                window.open_dialog(cx, move |dialog, _window, cx| {
                                                    dialog
                                                        .title("Compare changes")
                                                        .keyboard(true)
                                                        .child(
                                                            v_flex()
                                                                .gap_2()
                                                                .child(div().child(
                                                                    format!("Licoman (stored): {stored}"),
                                                                ))
                                                                .child(div().child(
                                                                    format!("Source (current): {external}"),
                                                                ))
                                                                .child(
                                                                    div()
                                                                        .text_color(cx.theme().muted_foreground)
                                                                        .text_size(px(metrics::TEXT_META))
                                                                        .child(
                                                                            "The source stays authoritative until you reload.",
                                                                        ),
                                                                ),
                                                        )
                                                });
                                            },
                                        )),
                                ),
                        ),
                )
                .into_any()
            }
        }
    }

    /// The "Reset to default…" action line for editable Idle rows whose
    /// backend offers a reset. The disclosure dialog explains what the reset
    /// concretely does before anything is dispatched
    /// (06_SAFE_WRITE_ENGINE.md §5).
    fn reset_action_line(&mut self, row: &Row, cx: &mut Context<Self>) -> Option<AnyElement> {
        let descriptor_control_readonly = row.descriptor.control == licoman_core::Control::ReadOnly;
        if descriptor_control_readonly {
            return None;
        }
        let support = self.app.read(cx).store.reset_support(&row.descriptor.id)?;
        if matches!(
            support.disclosure,
            licoman_core::ResetDisclosure::NotPossible { .. }
        ) {
            return None;
        }
        let reset_id = SharedString::from(format!("reset-{}", row.descriptor.id));
        let setting_id = row.descriptor.id.clone();
        let app = self.app.clone();
        let button = Button::new(reset_id)
            .label("Reset to default…")
            .ghost()
            .xsmall()
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                let setting_id = setting_id.clone();
                let support = this
                    .app
                    .read(cx)
                    .store
                    .reset_support(&setting_id)
                    .expect("checked when rendering the button");
                let explanation = support.disclosure.explanation();
                let confirm_id = SharedString::from(format!("reset-confirm-{setting_id}"));
                let app = app.clone();
                let confirm_id_inner = confirm_id.clone();
                window.open_dialog(cx, move |dialog, _window, cx| {
                    let theme = cx.theme().clone();
                    let setting_id = setting_id.clone();
                    let app = app.clone();
                    dialog.title("Reset to default").keyboard(true).child(
                        v_flex()
                            .gap_3()
                            .child(div().child(explanation.clone()))
                            .child(
                                div()
                                    .text_size(px(metrics::TEXT_META))
                                    .text_color(theme.muted_foreground)
                                    .child(
                                        "This writes through the safe write \
                                                     engine with a conflict check and a \
                                                     backup of the previous value.",
                                    ),
                            )
                            .child(
                                h_flex()
                                    .justify_end()
                                    .gap_2()
                                    .child(
                                        Button::new("reset-cancel")
                                            .label("Cancel")
                                            .ghost()
                                            .on_click(|_, window, cx| {
                                                window.close_dialog(cx);
                                            }),
                                    )
                                    .child(
                                        Button::new(confirm_id_inner.clone())
                                            .label("Reset")
                                            .primary()
                                            .on_click(move |_, window, cx| {
                                                window.close_dialog(cx);
                                                let setting_id = setting_id.clone();
                                                app.update(cx, |state, cx| {
                                                    state.dispatch(
                                                        Command::ResetToDefault { id: setting_id },
                                                        cx,
                                                    );
                                                });
                                            }),
                                    ),
                            ),
                    )
                });
            }));
        Some(h_flex().gap_2().child(button).into_any_element())
    }

    /// The small Revert button for rows holding an unapplied edit (used by
    /// the permission-required state, where the edit is retained but
    /// refused).
    fn revert_edit_button(&mut self, setting_id: &str, cx: &mut Context<Self>) -> Button {
        let revert_id = SharedString::from(format!("revert-edit-{setting_id}"));
        let setting_id = setting_id.to_string();
        Button::new(revert_id)
            .label("Discard edit")
            .ghost()
            .xsmall()
            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                let setting_id = setting_id.clone();
                this.app.update(cx, |state, cx| {
                    state.dispatch(Command::Revert { id: setting_id }, cx);
                });
            }))
    }

    /// The Idle status line for rows with read provenance: the value origin,
    /// the read-only rule when it applies, and a Source details disclosure.
    /// Nothing here carries configuration contents — only identifiers and
    /// metadata (05_READ_ONLY_BACKENDS.md §5).
    fn source_details_line(
        &mut self,
        row: &Row,
        theme: &gpui_kit::component::Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let read_meta = row.model.read_meta.as_ref()?;
        let origin = read_meta.origin.label().to_string();
        let read_only = matches!(read_meta.writable, licoman_core::read::Writable::No { .. });
        let details_button_id = SharedString::from(format!("details-{}", row.descriptor.id));
        let setting_id = row.descriptor.id.clone();
        let module_id = row.descriptor.module_id.clone();
        let app = self.app.clone();
        let read_only_note = read_only.then(|| " · this setting cannot be changed yet".to_string());
        Some(
            v_flex().gap_1().child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(metrics::TEXT_META))
                            .text_color(theme.muted_foreground)
                            .child(match &read_only_note {
                                Some(note) => format!("{origin}{note}"),
                                None => origin,
                            }),
                    )
                    .child(
                        Button::new(details_button_id)
                            .label("Source details…")
                            .ghost()
                            .xsmall()
                            .on_click(cx.listener(move |_this, _: &ClickEvent, window, cx| {
                                let (lines, provenance) =
                                    source_details_lines(&app, &setting_id, &module_id, cx);
                                window.open_dialog(cx, move |dialog, _window, cx| {
                                    let theme = cx.theme().clone();
                                    let panel =
                                        details_panel(lines.clone(), provenance.clone(), &theme);
                                    dialog.title("Source details").keyboard(true).child(panel)
                                });
                            })),
                    ),
            ),
        )
        .into_any()
    }
}

impl Render for SettingRowView {
    #[allow(refining_impl_trait)] // AnyElement is the concrete element type
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        crate::startup::note_first_frame();
        self.render_setting(window, cx)
    }
}

impl Render for MainWindow {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        if self.rebuild_needed {
            self.rebuild_controls(window, cx);
        } else if self.resync_needed {
            self.sync_owned_controls(window, cx);
        }
        if let Some(query) = self.pending_palette_query.take() {
            self.palette_open = true;
            self.palette_selected = 0;
            let focus = self.palette_input.read(cx).focus_handle(cx).clone();
            self.palette_input.update(cx, |state, cx| {
                state.set_value(query, window, cx);
            });
            window.focus(&focus, cx);
        }

        let theme = cx.theme().clone();

        v_flex()
            .key_context("MainWindow")
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .on_action(cx.listener(|this, _: &OpenSearch, window, cx| {
                this.open_palette(window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                let handle = this.header_search.read(cx).focus_handle(cx).clone();
                window.focus(&handle, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenPreferences, _window, cx| {
                this.open_preferences_window(cx);
            }))
            .on_action(cx.listener(|this, _: &OpenApplications, _window, cx| {
                this.open_applications_window(cx);
            }))
            .on_action(cx.listener(|this, _: &DismissPalette, window, cx| {
                if this.palette_open {
                    this.close_palette(window, cx);
                }
            }))
            .child(
                TitleBar::new().h(px(metrics::HEADER_HEIGHT)).child(
                    h_flex()
                        .w_full()
                        .h_full()
                        .items_center()
                        .gap_3()
                        .px_2()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(crate::brand::brand_mark().text_color(theme.primary))
                                .child(
                                    div()
                                        .text_size(px(metrics::TEXT_LABEL))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child("Licoman"),
                                ),
                        )
                        .child(
                            h_flex().flex_1().max_w(px(420.)).child(
                                Input::new(&self.header_search)
                                    .id("header-search")
                                    .prefix(Icon::new(IconName::Search).small())
                                    .cleanable(true),
                            ),
                        )
                        .child(
                            div()
                                .text_size(px(metrics::TEXT_META))
                                .text_color(theme.muted_foreground)
                                .child("Ctrl K"),
                        )
                        .child(
                            Button::new("open-preferences")
                                .icon(IconName::Settings)
                                .ghost()
                                .compact()
                                .tooltip("Licoman Preferences (Ctrl+,)")
                                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                                    this.open_preferences_window(cx);
                                })),
                        ),
                ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(self.render_sidebar(cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_h_0()
                            .border_l_1()
                            .border_color(theme.border)
                            .bg(theme.background)
                            .child(self.render_content(window, cx))
                            .children(self.render_apply_bar(cx)),
                    ),
            )
            .children(self.render_palette(cx))
    }
}
