//! Command-line surface: `--help`, `--version`, and argument errors. Pure and
//! unit tested; `main` maps the parsed outcome to stdout/stderr and exit
//! codes. No external argument parser is introduced for two flags.

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
         \x20       visual verification (documented in docs/handoffs).\n\
         \n\
         NOTES:\n\
         \x20   Licoman only ever writes its own preferences file\n\
         \x20   ($XDG_CONFIG_HOME/licoman/preferences.toml). No OS, desktop or\n\
         \x20   application configuration is modified in this build.\n\
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
        _ => Parsed::Fail(format!(
            "licoman: unrecognized argument '{arg}'\nTry 'licoman --help' for usage."
        )),
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
