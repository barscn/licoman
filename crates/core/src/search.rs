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
    use std::time::{Duration, Instant};

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

    /// Phase 08 scale gate: search latency over 1,000+ synthetic settings
    /// and 100+ synthetic apps. Asserts a generous bound (CI stability) and
    /// prints the measured percentiles for the handoff report (the plan's
    /// target is p95 < 100 ms on reference hardware — the actual figures
    /// are recorded per run, never invented).
    #[test]
    fn scale_search_latency_1000_settings_100_apps() {
        let mut descriptors = Vec::new();
        // 100 shared modules x 10 settings (1,000 settings) + 120 app
        // modules x 2 settings (240) = 1,240 synthetic descriptors.
        for module in 0..100 {
            for index in 0..10 {
                descriptors.push(SettingDescriptor::new(
                    format!("shared.scale.{module:03}.setting_{index:02}"),
                    Scope::Shared,
                    "ScaleCatalog",
                    format!("Scale setting {module:03}/{index:02}"),
                    "Synthetic scale-catalog setting; mock-backed declared default.",
                    crate::value::SettingType::Bool,
                    Control::Switch,
                ));
            }
        }
        for module in 0..120 {
            for index in 0..2 {
                descriptors.push(SettingDescriptor::new(
                    format!("app.scale.{module:03}.option_{index:02}"),
                    Scope::Application,
                    "ScaleApps",
                    format!("Scale option {module:03}/{index:02}"),
                    "Synthetic application setting; mock-backed declared default.",
                    crate::value::SettingType::Bool,
                    Control::Switch,
                ));
            }
        }
        assert_eq!(descriptors.len(), 1240);

        let build_started = Instant::now();
        let index = SearchIndex::build(descriptors.iter());
        let build_elapsed = build_started.elapsed();

        // Realistic query mix: prefixes, mid-word terms, ids, app titles,
        // misses.
        let queries = [
            "scale setting",
            "scale option",
            "setting 0",
            "option 1",
            "ScaleCatalog",
            "ScaleApps",
            "shared.scale",
            "app.scale",
            "setting 037",
            "option 119",
            "synthetic",
            "zzz-no-such-term",
        ];
        let runs = 3;
        let mut per_query: Vec<Duration> = Vec::new();
        for _ in 0..runs {
            for query in queries {
                let started = Instant::now();
                let hits = index.search(query);
                let elapsed = started.elapsed();
                assert!(
                    !hits.is_empty() || query == "zzz-no-such-term",
                    "query {query:?} unexpectedly matched nothing"
                );
                per_query.push(elapsed);
            }
        }
        per_query.sort();
        let p50 = per_query[per_query.len() / 2];
        let p95 = per_query[(per_query.len() as f64 * 0.95) as usize];
        println!(
            "scale search: index build {:?}; {} queries x {runs} runs; p50 {:?}; p95 {:?}; max {:?}",
            build_elapsed,
            queries.len(),
            p50,
            p95,
            per_query[per_query.len() - 1]
        );
        // Generous CI bound: the plan's p95 target is 100 ms on reference
        // hardware; a 50x multiple keeps the assertion stable everywhere.
        assert!(p95 < Duration::from_millis(100) * 50);
        assert!(build_elapsed < Duration::from_secs(10));
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
