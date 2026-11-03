//! The scale catalog (`LICOMAN_DEMO=scale`): a synthetic, purely mock-backed
//! module set for the Phase 08 performance gates — 1,000+ settings and 100+
//! applications rendered and searched through the real registry/search/UI
//! paths. Nothing here touches a backend: every setting is `mock`-backed
//! with a declared default, so the demo is isolated by construction.

/// Deterministic mock-backed manifests: `shared_count` shared modules with
/// `per_shared` settings each (the main-window scroll surface) and
/// `app_count` application modules with `per_app` settings each (the
/// Applications-window surface).
pub fn manifests(
    shared_count: usize,
    per_shared: usize,
    app_count: usize,
    per_app: usize,
) -> Vec<(String, String)> {
    let mut out = Vec::with_capacity(shared_count + app_count);
    for module in 0..shared_count {
        out.push((
            format!("shared.scale.{module:03}"),
            shared_manifest(module, per_shared),
        ));
    }
    for module in 0..app_count {
        out.push((
            format!("app.scale.{module:03}"),
            app_manifest(module, per_app),
        ));
    }
    out
}

fn shared_manifest(module: usize, per_module: usize) -> String {
    let mut settings = String::new();
    for index in 0..per_module {
        let (control, kind, default) = if index % 3 == 0 {
            ("switch", "boolean", "true".to_string())
        } else if index % 3 == 1 {
            (
                "text",
                "string",
                format!("\"sample value {module}-{index}\""),
            )
        } else {
            (
                "read_only",
                "string",
                format!("\"declared fact {module}-{index}\""),
            )
        };
        let policy = if index % 3 == 2 {
            "read_only"
        } else {
            "staged"
        };
        settings.push_str(&format!(
            r#"
[[settings]]
id = "setting_{index:02}"
title = "Scale setting {module:03}/{index:02}"
description = "Synthetic scale-catalog setting; mock-backed declared default."
keywords = ["scale", "synthetic", "catalog"]
type = "{kind}"
control = "{control}"
policy = "{policy}"
default = {default}

[settings.backend]
kind = "mock"
"#
        ));
    }
    format!(
        r#"schema_version = 1
id = "shared.scale.{module:03}"
scope = "shared"
title = "Scale module {module:03}"
description = "Synthetic scale-catalog module (mock-backed)."
category = "ScaleCatalog"
order = {order}

{settings}
"#,
        order = 1000 + module,
    )
}

fn app_manifest(module: usize, per_module: usize) -> String {
    let mut settings = String::new();
    for index in 0..per_module {
        settings.push_str(&format!(
            r#"
[[settings]]
id = "option_{index:02}"
title = "Scale option {module:03}/{index:02}"
description = "Synthetic application setting; mock-backed declared default."
keywords = ["scale", "synthetic", "app"]
type = "boolean"
control = "switch"
default = {default}
group = "Options"
order = {index}

[settings.backend]
kind = "mock"
"#,
            default = index % 2 == 0,
        ));
    }
    format!(
        r#"schema_version = 1
id = "app.scale.{module:03}"
scope = "application"
title = "Scale application {module:03}"
description = "Synthetic scale-catalog application module (mock-backed)."
category = "ScaleApps"
order = {order}

{settings}
"#,
        order = 1000 + module,
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn generated_manifests_are_valid() {
        let manifests = super::manifests(2, 4, 2, 2);
        for (name, text) in &manifests {
            let parsed = licoman_core::modules::manifest::parse_manifest(text)
                .unwrap_or_else(|error| panic!("{name}: {error:?}"));
            let issues = licoman_core::modules::manifest::validate_definition(&parsed);
            assert!(
                issues
                    .iter()
                    .all(|issue| issue.severity
                        != licoman_core::modules::manifest::IssueSeverity::Error),
                "{name}: {issues:?}"
            );
        }
    }
}
