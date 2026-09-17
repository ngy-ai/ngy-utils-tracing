//! Guards the "single source of truth" rule for the environment-variable fallbacks.
//!
//! The authoritative description of every option ↔ environment variable ↔ default mapping is the
//! `InitOptions` field docs; `README.md` keeps a short quick-reference table and `AGENTS.md` only
//! points at it. These tests fail when the copies drift apart, when a variable disappears from the
//! authoritative docs, or when the required `env_prefix` is not reflected in the README.

use ngy_utils_tracing::{
    AppMode, DEFAULT_LOG_DIR, DEFAULT_LOG_LEVEL, DEFAULT_LOG_PREFIX, DEFAULT_MAX_LOG_FILES,
    DEFAULT_TIME_OFFSET,
};

/// Every environment variable the crate reads.
///
/// These are the *suffixes* documented on `InitOptions`; at runtime each name is prefixed with the
/// caller's `InitOptions::env_prefix`, except `RUST_LOG` (see [`documented_name`]).
const SUPPORTED_VARS: [&str; 6] = [
    "APP_MODE",
    "RUST_LOG",
    "LOG_DIR",
    "LOG_PREFIX",
    "LOG_MAX_FILES",
    "LOG_TIME_OFFSET",
];

/// Name as it must appear in the README: `RUST_LOG` unprefixed, everything else with the marker.
fn documented_name(suffix: &str) -> String {
    if suffix == "RUST_LOG" {
        suffix.to_string()
    } else {
        format!("{{prefix}}{suffix}")
    }
}

/// Parse the "Environment Variables" table of `README.md` into `(variable, default)` pairs.
fn readme_env_table() -> Vec<(String, String)> {
    let readme = include_str!("../README.md");
    let mut table = Vec::new();
    let mut in_section = false;

    for line in readme.lines() {
        if line.starts_with("## ") {
            in_section = line.starts_with("## Environment Variables");
            continue;
        }
        if !in_section || !line.starts_with('|') {
            continue;
        }

        // Rows look like `| `VAR` | `default` | purpose |`; the outer splits are empty.
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        let (Some(var), Some(default)) = (cells.get(1), cells.get(2)) else {
            continue;
        };
        // Skips the column-title row and the `---` separator row.
        if var.is_empty() || *var == "Variable" || var.starts_with('-') {
            continue;
        }
        table.push((
            var.trim_matches('`').to_string(),
            default.trim_matches('`').to_string(),
        ));
    }

    table
}

#[test]
fn readme_env_table_documents_exactly_the_supported_variables() {
    let documented: Vec<String> = readme_env_table().into_iter().map(|(var, _)| var).collect();
    let expected: Vec<String> = SUPPORTED_VARS
        .iter()
        .map(|var| documented_name(var))
        .collect();

    assert_eq!(
        documented, expected,
        "the README environment table must list exactly these variables, in this order, with the \
         `{{prefix}}` marker on every one except `RUST_LOG`"
    );
}

#[test]
fn readme_env_table_defaults_match_the_code_constants() {
    let expected = [
        ("APP_MODE", AppMode::default().as_str().to_string()),
        ("RUST_LOG", DEFAULT_LOG_LEVEL.to_string()),
        ("LOG_DIR", DEFAULT_LOG_DIR.to_string()),
        ("LOG_PREFIX", DEFAULT_LOG_PREFIX.to_string()),
        ("LOG_MAX_FILES", DEFAULT_MAX_LOG_FILES.to_string()),
        ("LOG_TIME_OFFSET", DEFAULT_TIME_OFFSET.to_string()),
    ];

    let table = readme_env_table();
    for (suffix, expected_default) in expected {
        let name = documented_name(suffix);
        let default = table
            .iter()
            .find(|(var, _)| *var == name)
            .map(|(_, default)| default)
            .unwrap_or_else(|| panic!("`{name}` is missing from the README environment table"));
        assert_eq!(
            default, &expected_default,
            "README documents `{name}` with a default that no longer matches the code"
        );
    }
}

#[test]
fn readme_env_section_explains_the_required_prefix() {
    let readme = include_str!("../README.md");

    assert!(
        readme.contains("env_prefix"),
        "the README must explain that every variable is read with `InitOptions::env_prefix`"
    );
}

#[test]
fn init_options_docs_name_every_supported_variable() {
    // `src/init.rs` holds the authoritative field docs, so each variable must be named there.
    let source = include_str!("../src/init.rs");

    for var in SUPPORTED_VARS {
        assert!(
            source.contains(var),
            "`{var}` is not documented in the `InitOptions` field docs"
        );
    }
}

#[test]
fn agents_md_points_at_the_authoritative_docs_instead_of_copying_the_table() {
    let agents = include_str!("../AGENTS.md");

    assert!(
        agents.contains("src/init.rs"),
        "AGENTS.md should point at the authoritative `InitOptions` docs"
    );
    assert!(
        !agents.contains("| `APP_MODE`"),
        "AGENTS.md must not keep a second copy of the environment-variable table"
    );
}
