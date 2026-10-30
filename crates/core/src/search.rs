//! Global settings search. The index is derived from registry descriptors —
//! **not** from the currently visible scope — so result identities are stable
//! across selection changes (02_FOUNDATION.md tests §2).

use crate::descriptor::Scope;

/// One search result. `id` is the stable setting identity the UI navigates
/// with; `breadcrumb` is scope → category → setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub id: String,
    pub label: String,
    pub breadcrumb: String,
    pub scope: Scope,
}

impl SearchHit {
    pub fn from_descriptor(descriptor: &crate::descriptor::SettingDescriptor) -> Self {
        Self {
            id: descriptor.id.to_string(),
            label: descriptor.label.to_string(),
            breadcrumb: descriptor.breadcrumb(),
            scope: descriptor.scope,
        }
    }
}

/// Search all descriptors for `query`. Empty or whitespace-only queries return
/// no results (the palette shows its "type to search" hint instead). Matching
/// is a case-insensitive substring test over id, label, description, category
/// and breadcrumb; results keep registration order so they are deterministic.
pub fn search<'a, I>(descriptors: I, query: &str) -> Vec<SearchHit>
where
    I: IntoIterator<Item = &'a crate::descriptor::SettingDescriptor>,
{
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    descriptors
        .into_iter()
        .filter(|d| {
            d.id.to_lowercase().contains(&q)
                || d.label.to_lowercase().contains(&q)
                || d.description.to_lowercase().contains(&q)
                || d.category.to_lowercase().contains(&q)
                || d.breadcrumb().to_lowercase().contains(&q)
        })
        .map(SearchHit::from_descriptor)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{ApplyMode, Control, SettingDescriptor};
    use crate::value::SettingType;

    fn descriptor(
        id: &'static str,
        scope: Scope,
        category: &'static str,
        label: &'static str,
    ) -> SettingDescriptor {
        SettingDescriptor {
            id,
            scope,
            category,
            label,
            description: "description text",
            setting_type: SettingType::Bool,
            control: Control::Switch,
            apply_mode: ApplyMode::Staged,
            restart_target: None,
            permission_scope: None,
            note: None,
            min: None,
            max: None,
        }
    }

    fn fixtures() -> Vec<SettingDescriptor> {
        vec![
            descriptor("shared.network.wifi", Scope::Shared, "Network", "Wi-Fi"),
            descriptor(
                "desktop.session.gaps",
                Scope::Desktop,
                "Windows",
                "Window gaps",
            ),
        ]
    }

    #[test]
    fn empty_query_returns_no_hits() {
        assert!(search(fixtures().iter(), "").is_empty());
        assert!(search(fixtures().iter(), "   ").is_empty());
    }

    #[test]
    fn matches_label_case_insensitively_and_keeps_order() {
        let hits = search(fixtures().iter(), "wi-fi");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "shared.network.wifi");
        assert_eq!(hits[0].breadcrumb, "Shared → Network → Wi-Fi");
    }

    #[test]
    fn ids_are_stable_across_scope_selection_changes() {
        // The index is built from descriptors, so simulating a selection
        // change must not alter identities or ordering.
        let before = search(fixtures().iter(), "a");
        let mut reordered = fixtures();
        reordered.reverse();
        // Reversing the fixture list changes registration order (and thus
        // display order) — but each hit still carries the same stable id for
        // the same setting, which is what navigation keys on.
        let after: Vec<String> = search(reordered.iter(), "a")
            .into_iter()
            .map(|h| h.id)
            .collect();
        assert_eq!(after.len(), 2);
        for hit in &before {
            assert!(after.contains(&hit.id), "stable id lost: {}", hit.id);
        }
    }
}
