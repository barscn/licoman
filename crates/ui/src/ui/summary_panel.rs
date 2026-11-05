//! The Summary panel — the main window's landing view: distribution logo,
//! OS name, then the machine fact list in the requested order (kernel,
//! uptime, shell, window manager, terminal, CPU, GPUs, memory, swap when
//! configured, root-disk usage). All facts come from the fail-closed
//! [`licoman_core::system::SystemFacts`] cache; unknown facts render as
//! "unknown", never placeholder values, and the whole panel says
//! "Collecting system facts…" until the first background pass lands.

use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::theme::Theme;
use gpui_kit::component::{Sizable as _, Size, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, FontWeight, InteractiveElement as _, IntoElement as _, ParentElement as _, Styled,
    div, img, px,
};
use licoman_core::descriptor::Scope;
use licoman_core::system::{format_uptime, human_bytes};

use crate::app_state::AppState;
use crate::brand;
use crate::theme::metrics;
use crate::ui::window_heading;

/// Render the Summary content area (a pure function of the app state — no
/// inputs, so unlike the account panel this is not an entity).
pub fn render(state: &AppState, theme: &Theme) -> AnyElement {
    let facts = state.system_facts();

    // Hero: the distribution logo (or Licoman's mark as the honest fallback)
    // beside the OS display name.
    let logo: AnyElement = match facts.and_then(|facts| facts.logo.clone()) {
        Some(path) => img(path).size(px(80.)).flex_shrink_0().into_any_element(),
        None => brand::brand_mark()
            .text_color(theme.primary)
            .with_size(Size::Size(px(80.)))
            .into_any_element(),
    };
    let os_name = facts
        .and_then(|facts| facts.os_name.clone())
        .unwrap_or_else(|| "Operating system unknown".to_string());

    let mut entries: Vec<(&'static str, String)> = Vec::new();
    if let Some(facts) = facts {
        let unknown = || "unknown".to_string();
        entries.push((
            "Kernel",
            facts.kernel_release.clone().unwrap_or_else(unknown),
        ));
        entries.push((
            "Uptime",
            facts.uptime_secs.map(format_uptime).unwrap_or_else(unknown),
        ));
        entries.push(("Shell", facts.shell.clone().unwrap_or_else(unknown)));
        entries.push((
            "Window Manager",
            facts.window_manager.clone().unwrap_or_else(unknown),
        ));
        entries.push(("Terminal", facts.terminal.clone().unwrap_or_else(unknown)));
        entries.push(("CPU", cpu_value(facts)));
        for (index, gpu) in facts.gpus.iter().take(2).enumerate() {
            entries.push((gpu_label(index), gpu_value(gpu)));
        }
        entries.push(("Memory", memory_value(facts)));
        if let Some(swap) = &facts.swap {
            entries.push(("Swap", swap_value(swap)));
        }
        entries.push(("Disk", disk_value(facts)));
    }

    v_flex()
        .flex_1()
        .min_h_0()
        .id("summary-content")
        .px(px(metrics::CONTENT_PADDING))
        .py_5()
        .gap_4()
        .overflow_y_scrollbar()
        .child(
            v_flex()
                .max_w(px(metrics::CONTENT_MAX_WIDTH))
                .gap_3()
                .child(window_heading(Scope::Summary.label(), theme))
                .child(
                    div()
                        .text_size(px(metrics::TEXT_BODY))
                        .text_color(theme.muted_foreground)
                        .child(Scope::Summary.description()),
                ),
        )
        .child(
            // The hero: logo + OS name. Read from the same snapshot as the
            // rows, so it never contradicts them.
            h_flex()
                .max_w(px(metrics::CONTENT_MAX_WIDTH))
                .items_center()
                .gap_4()
                .py_2()
                .child(logo)
                .child(
                    v_flex()
                        .min_w_0()
                        .gap_0p5()
                        .child(
                            div()
                                .text_size(px(metrics::TEXT_PAGE_HEADING))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.foreground)
                                .child(os_name),
                        )
                        .child(
                            div()
                                .text_size(px(metrics::TEXT_META))
                                .text_color(theme.muted_foreground)
                                .child("This machine, as read from /proc, /sys and os-release."),
                        ),
                ),
        )
        .child(
            v_flex()
                .max_w(px(metrics::CONTENT_MAX_WIDTH))
                .when(facts.is_none(), |list| {
                    list.child(
                        div()
                            .text_size(px(metrics::TEXT_BODY))
                            .text_color(theme.muted_foreground)
                            .py_2()
                            .child("Collecting system facts…"),
                    )
                })
                .children(
                    entries
                        .into_iter()
                        .map(|(label, value)| fact_row(label, value, theme)),
                ),
        )
        .into_any_element()
}

/// One fact row: muted label left, value right, hairline underneath — the
/// same visual language as the settings rows without pretending to be one.
fn fact_row(label: &'static str, value: String, theme: &Theme) -> AnyElement {
    h_flex()
        .justify_between()
        .items_baseline()
        .py_2()
        .border_b_1()
        .border_color(theme.border)
        .child(
            div()
                .text_size(px(metrics::TEXT_BODY))
                .text_color(theme.muted_foreground)
                .child(label),
        )
        .child(
            div()
                .text_size(px(metrics::TEXT_BODY))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.foreground)
                .child(value),
        )
        .into_any_element()
}

fn cpu_value(facts: &licoman_core::system::SystemFacts) -> String {
    match &facts.cpu {
        Some(cpu) => {
            let cores = match cpu.logical_cores {
                1 => "1 logical core".to_string(),
                cores => format!("{cores} logical cores"),
            };
            match &cpu.model {
                Some(model) => format!("{model} · {cores}"),
                None => cores,
            }
        }
        None => "unknown".to_string(),
    }
}

fn gpu_label(index: usize) -> &'static str {
    match index {
        0 => "GPU 1",
        1 => "GPU 2",
        _ => "GPU",
    }
}

fn gpu_value(gpu: &licoman_core::system::GpuInfo) -> String {
    let name = match (gpu.vendor, &gpu.model) {
        (Some(vendor), Some(model)) => format!("{vendor} {model}"),
        (Some(vendor), None) => format!("{vendor} ({})", gpu.pci_id),
        (None, Some(model)) => model.clone(),
        (None, None) => format!("GPU ({})", gpu.pci_id),
    };
    match gpu.kind {
        Some(licoman_core::system::GpuKind::Integrated) => format!("{name} · Integrated"),
        Some(licoman_core::system::GpuKind::Discrete) => format!("{name} · Discrete"),
        None => name,
    }
}

fn memory_value(facts: &licoman_core::system::SystemFacts) -> String {
    match &facts.memory {
        Some(memory) => format!(
            "{} / {} used",
            human_bytes(memory.used_bytes),
            human_bytes(memory.total_bytes)
        ),
        None => "unknown".to_string(),
    }
}

fn swap_value(swap: &licoman_core::system::SwapInfo) -> String {
    format!(
        "{} / {} used",
        human_bytes(swap.used_bytes),
        human_bytes(swap.total_bytes)
    )
}

fn disk_value(facts: &licoman_core::system::SystemFacts) -> String {
    match &facts.disk {
        Some(disk) => format!(
            "{} / {} used (/)",
            human_bytes(disk.used_bytes),
            human_bytes(disk.total_bytes)
        ),
        None => "unknown".to_string(),
    }
}
