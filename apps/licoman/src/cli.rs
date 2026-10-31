//! Command-line surface: `--help`, `--version`, `--diagnostics`, the
//! Phase 04 module commands (`module validate`, `module schema`), and
//! argument errors. Pure and unit tested; `main` maps the parsed outcome to
//! stdout/stderr and exit codes. No external argument parser is introduced.

/// The version string reported by `--version`.
pub fn version_string() -> String {
    format!("licoman {}", env!("CARGO_PKG_VERSION"))
}

/// The `--help` text.
pub fn help_text() -> String {
    format!(
        "Licoman — Linux Configuration Manager\n\
         \n\
         A universal interface for Linux configuration: one native desktop\n\
         application over shared/system, desktop-environment, distribution and\n\
         installed-application settings.\n\
         \n\
         USAGE:\n\
         \x20   licoman [OPTIONS]\n\
         \n\
         OPTIONS:\n\
         \x20   -h, --help       Print this help and exit\n\
         \x20   -V, --version    Print version information and exit\n\
         \x20   --diagnostics    Print the redacted environment diagnostics and exit\n\
         \n\
         MODULE COMMANDS:\n\
         \x20   licoman module validate <path>...\n\
         \x20       Read-only manifest linter: parses and validates module\n\
         \x20       files (or *.toml inside directories) and reports located\n\
         \x20       diagnostics plus applicability against this environment.\n\
         \x20       Never modifies anything.\n\
         \x20   licoman module schema\n\
         \x20       Print the JSON Schema generated from Licoman's canonical\n\
         \x20       module types (for editors and CI validation).\n\
         \n\
         ENVIRONMENT:\n\
         \x20   WAYLAND_DISPLAY, DISPLAY\n\
         \x20       Graphical session to connect to. Without one, Licoman exits\n\
         \x20       with a diagnostic instead of opening a broken window.\n\
         \x20   LICOMAN_LOG\n\
         \x20       Log level for structured stderr logs: error, warn (default),\n\
         \x20       info, debug.\n\
         \x20   LICOMAN_DEMO\n\
         \x20       Development hook that opens a deterministic screen for\n\
         \x20       visual verification (documented in docs/handoffs):\n\
         \x20       reads, reads-power and reads-apps exercise the Phase 05\n\
         \x20       read-only backends against fixture data; writes,\n\
         \x20       writes-conflict, writes-permission and writes-light\n\
         \x20       exercise the Phase 06 safe write engine against a temp\n\
         \x20       fixture copy and a fake D-Bus service (nothing outside\n\
         \x20       the temp directory is touched).\n\
         \n\
         NOTES:\n\
         \x20   Licoman only ever writes its own preferences file\n\
         \x20   ($XDG_CONFIG_HOME/licoman/preferences.toml). Real settings are\n\
         \x20   read (GSettings, config files, D-Bus services) but never\n\
         \x20   modified in this build: writes land only in the Phase 06\n\
         \x20   fixture demos (a throwaway temp copy), never in a live run.\n\
         \n\
         Version {}",
        env!("CARGO_PKG_VERSION")
    )
}

/// Outcome of parsing the process arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// Launch the application.
    Run,
    /// Run headless discovery and print the redacted diagnostics export.
    Diagnostics,
    /// Validate module manifests (read-only linter); never modifies anything.
    ModuleValidate(Vec<String>),
    /// Print the JSON Schema for module manifests.
    ModuleSchema,
    /// Print text to stdout and exit 0.
    PrintAndSucceed(String),
    /// Print text to stderr and exit 2.
    Fail(String),
}

/// Parse arguments (excluding argv[0]). The first argument decides the
/// outcome: a recognized flag acts immediately, anything else is an error.
pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Parsed {
    let mut args = args.into_iter();
    let Some(arg) = args.next() else {
        return Parsed::Run;
    };
    match arg.as_str() {
        "-h" | "--help" => Parsed::PrintAndSucceed(help_text()),
        "-V" | "--version" => Parsed::PrintAndSucceed(version_string()),
        "--diagnostics" => Parsed::Diagnostics,
        "module" => parse_module_subcommand(args.collect()),
        _ => Parsed::Fail(format!(
            "licoman: unrecognized argument '{arg}'\nTry 'licoman --help' for usage."
        )),
    }
}

/// Parse `licoman module <subcommand> [args…]`.
fn parse_module_subcommand(rest: Vec<String>) -> Parsed {
    let mut rest = rest.into_iter();
    match rest.next().as_deref() {
        Some("validate") => {
            let paths: Vec<String> = rest.collect();
            if paths.is_empty() {
                Parsed::Fail(
                    "licoman: 'module validate' needs at least one manifest file or directory\n                     Usage: licoman module validate <path>..."
                        .to_string(),
                )
            } else {
                Parsed::ModuleValidate(paths)
            }
        }
        Some("schema") => Parsed::ModuleSchema,
        Some(other) => Parsed::Fail(format!(
            "licoman: unknown module subcommand '{other}'\n             Try 'licoman module validate <path>...' or 'licoman module schema'."
        )),
        None => Parsed::Fail(
            "licoman: 'module' needs a subcommand\n\
             Usage: licoman module validate <path>... | licoman module schema"
                .to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_arguments_runs() {
        assert_eq!(parse(args(&[])), Parsed::Run);
    }

    #[test]
    fn help_and_version_print_and_succeed() {
        let Parsed::PrintAndSucceed(help) = parse(args(&["--help"])) else {
            panic!("expected help");
        };
        assert!(help.contains("Licoman — Linux Configuration Manager"));
        assert!(help.contains("--version"));
        let Parsed::PrintAndSucceed(version) = parse(args(&["-V"])) else {
            panic!("expected version");
        };
        assert_eq!(version, version_string());
        assert!(version.starts_with("licoman "));
        assert!(!version.contains('\n'));
    }

    #[test]
    fn diagnostics_flag_is_recognized() {
        assert_eq!(parse(args(&["--diagnostics"])), Parsed::Diagnostics);
    }

    #[test]
    fn unknown_argument_fails_with_guidance() {
        assert_eq!(
            parse(args(&["--bogus"])),
            Parsed::Fail(
                "licoman: unrecognized argument '--bogus'\nTry 'licoman --help' for usage."
                    .to_string()
            )
        );
        assert!(matches!(parse(args(&["extra"])), Parsed::Fail(_)));
    }

    #[test]
    fn module_subcommands_parse() {
        assert_eq!(parse(args(&["module", "schema"])), Parsed::ModuleSchema);
        assert_eq!(
            parse(args(&["module", "validate", "a.toml", "dir"])),
            Parsed::ModuleValidate(vec!["a.toml".to_string(), "dir".to_string()])
        );
        assert!(matches!(parse(args(&["module"])), Parsed::Fail(_)));
        assert!(matches!(
            parse(args(&["module", "validate"])),
            Parsed::Fail(_)
        ));
        assert!(matches!(parse(args(&["module", "bogus"])), Parsed::Fail(_)));
    }

    #[test]
    fn help_wins_over_later_arguments() {
        assert!(matches!(
            parse(args(&["--bogus", "--help"])),
            Parsed::Fail(_)
        ));
        assert!(matches!(
            parse(args(&["--help", "--bogus"])),
            Parsed::PrintAndSucceed(_)
        ));
    }
}
