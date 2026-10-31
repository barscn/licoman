//! Global settings search (02_FOUNDATION.md tests §2, extended in Phase 04).
//!
//! The index is a **precomputed, normalized text table** built from registry
//! descriptors — not from the currently visible scope — so result identities
//! are stable across selection changes and keystrokes never trigger list
//! reconstruction (04_MODULE_SCHEMA_AND_REGISTRY.md §6). Each entry packs the
//! setting id, label, description, category, breadcrumb, declared keywords
//! and provenance (module id) into one lowercase blob scanned per query.

use crate::descriptor::{Scope, SettingDescriptor};

/// One search result. `id` is the stable setting identity the UI navigates
/// with; `breadcrumb` is scope → category → setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub id: String,
    pub label: String,
    pub breadcrumb: String,
    pub scope: Scope,
    /// The module that declared the setting (provenance for navigation).
    pub module_id: String,
}

impl SearchHit {
    pub fn from_descriptor(descriptor: &SettingDescriptor) -> Self {
        Self {
            id: descriptor.id.clone(),
            label: descriptor.label.clone(),
            breadcrumb: descriptor.breadcrumb(),
            scope: descriptor.scope,
            module_id: descriptor.module_id.clone(),
        }
    }
}

/// Precomputed search index over a descriptor list.
#[derive(Debug, Clone, Default)]
pub struct SearchIndex {
    entries: Vec<(String, SearchHit)>,
}

impl SearchIndex {
    /// Build the normalized-text index once per registry load.
    pub fn build<'a, I: IntoIterator<Item = &'a SettingDescriptor>>(descriptors: I) -> Self {
        let mut entries: Vec<(String, SearchHit)> = descriptors
            .into_iter()
            .map(|d| {
                let mut blob = String::with_capacity(256);
                blob.push_str(&d.id.to_lowercase());
                blob.push(' ');
                blob.push_str(&d.label.to_lowercase());
                blob.push(' ');
                blob.push_str(&d.description.to_lowercase());
                blob.push(' ');
                blob.push_str(&d.category.to_lowercase());
                blob.push(' ');
                blob.push_str(&d.module_id.to_lowercase());
                for keyword in &d.keywords {
                    blob.push(' ');
                    blob.push_str(&keyword.to_lowercase());
                }
                blob.push(' ');
                blob.push_str(&d.breadcrumb().to_lowercase());
                (blob, SearchHit::from_descriptor(d))
            })
            .collect();
        entries.shrink_to_fit();
        Self { entries }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Search for `query`. Empty or whitespace-only queries return no results
    /// (the palette shows its "type to search" hint instead). Matching is a
    /// case-insensitive substring test over the precomputed text; results
    /// keep registration order so they are deterministic.
    pub fn search(&self, query: &str) -> Vec<SearchHit> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        self.entries
            .iter()
            .filter(|(blob, _)| blob.contains(&q))
            .map(|(_, hit)| hit.clone())
            .collect()
    }
}

/// Convenience wrapper: build an index over `descriptors` and search once.
/// Kept for callers that search a fresh list; hot paths should hold a
/// [`SearchIndex`] (see [`crate::model::Store`]).
pub fn search<'a, I>(descriptors: I, query: &str) -> Vec<SearchHit>
where
    I: IntoIterator<Item = &'a SettingDescriptor>,
{
    SearchIndex::build(descriptors).search(query)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{ApplyMode, Control, SettingDescriptor};
    use crate::value::SettingType;

    fn descriptor(
        id: &str,
        scope: Scope,
        category: &str,
        label: &str,
        keywords: &[&str],
    ) -> SettingDescriptor {
        SettingDescriptor {
            id: id.to_string(),
            scope,
            category: category.to_string(),
            label: label.to_string(),
            description: "description text".to_string(),
            setting_type: SettingType::Bool,
            control: Control::Switch,
            apply_mode: ApplyMode::Staged,
            restart_target: None,
            permission_scope: None,
            note: None,
            min: None,
            max: None,
            keywords: keywords.iter().map(|s| s.to_string()).collect(),
            module_id: "desktop.test".to_string(),
        }
    }

    fn fixtures() -> Vec<SettingDescriptor> {
        vec![
            descriptor(
                "shared.network.wifi",
                Scope::Shared,
                "Network",
                "Wi-Fi",
                &[],
            ),
            descriptor(
                "desktop.test.gaps",
                Scope::Desktop,
                "Windows",
                "Window gaps",
                &["borders", "tiling"],
            ),
        ]
    }

    #[test]
    fn empty_query_returns_no_hits() {
        let index = SearchIndex::build(fixtures().iter());
        assert!(index.search("").is_empty());
        assert!(index.search("   ").is_empty());
    }

    #[test]
    fn matches_label_case_insensitively_and_keeps_order() {
        let index = SearchIndex::build(fixtures().iter());
        let hits = index.search("wi-fi");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "shared.network.wifi");
        assert_eq!(hits[0].breadcrumb, "Shared → Network → Wi-Fi");
    }

    #[test]
    fn keywords_and_provenance_are_searchable() {
        let index = SearchIndex::build(fixtures().iter());
        assert_eq!(index.search("tiling").len(), 1);
        assert_eq!(index.search("tiling")[0].id, "desktop.test.gaps");
        assert_eq!(index.search("desktop.test")[0].module_id, "desktop.test");
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

    #[test]
    fn index_is_built_once_and_scanned_per_query() {
        // 1,200 synthetic entries: building is one pass, queries only scan
        // precomputed blobs (no per-keystroke descriptor re-walking).
        let descriptors: Vec<SettingDescriptor> = (0..1_200)
            .map(|i| {
                descriptor(
                    &format!("synthetic.group.setting{i}"),
                    Scope::Shared,
                    "Synthetic",
                    &format!("Synthetic setting {i}"),
                    &[],
                )
            })
            .collect();
        let index = SearchIndex::build(descriptors.iter());
        assert_eq!(index.len(), 1_200);
        let hits = index.search("setting999");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "synthetic.group.setting999");
        assert_eq!(index.search("synthetic").len(), 1_200);
    }
}
