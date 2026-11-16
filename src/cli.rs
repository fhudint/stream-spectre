//! Command-line argument parsing.

use std::ffi::OsString;
use std::path::PathBuf;

pub const HELP: &str = "\
stream-spectre: OBS browser-source stream overlay

USAGE:
    stream-spectre [OPTIONS]

OPTIONS:
    -c, --config <PATH>    Path to config.toml
    -h, --help             Print this help
    -V, --version          Print version

CONFIG LOOKUP (when --config is not given):
    $STREAM_SPECTRE_CONFIG, then ./config.toml, then the per-user config directory.

SECRETS (environment variables or a .env file):
    YOUTUBE_API_KEY        required
    DASHBOARD_TOKEN        required when the server binds beyond 127.0.0.1
";

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Run { config: Option<PathBuf> },
    Help,
    Version,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum CliError {
    #[error("unknown argument: {0}")]
    UnknownArgument(String),
    #[error("--config needs a path")]
    MissingConfigPath,
}

/// Parses arguments (without the program name).
pub fn parse<I>(args: I) -> Result<Command, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut config = None;
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("-h" | "--help") => return Ok(Command::Help),
            Some("-V" | "--version") => return Ok(Command::Version),
            Some("-c" | "--config") => {
                let path = args.next().ok_or(CliError::MissingConfigPath)?;
                config = Some(PathBuf::from(path));
            }
            _ => {
                return Err(CliError::UnknownArgument(
                    arg.to_string_lossy().into_owned(),
                ));
            }
        }
    }

    Ok(Command::Run { config })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn no_args_runs_with_default_config_lookup() {
        assert_eq!(parse(args(&[])), Ok(Command::Run { config: None }));
    }

    #[test]
    fn config_flag_takes_a_path() {
        for flag in ["-c", "--config"] {
            assert_eq!(
                parse(args(&[flag, "/tmp/c.toml"])),
                Ok(Command::Run {
                    config: Some(PathBuf::from("/tmp/c.toml"))
                })
            );
        }
        assert_eq!(parse(args(&["--config"])), Err(CliError::MissingConfigPath));
    }

    #[test]
    fn help_and_version_short_circuit() {
        assert_eq!(parse(args(&["--help"])), Ok(Command::Help));
        assert_eq!(parse(args(&["-V", "--bogus"])), Ok(Command::Version));
    }

    #[test]
    fn unknown_arguments_are_errors() {
        assert_eq!(
            parse(args(&["--nope"])),
            Err(CliError::UnknownArgument("--nope".to_owned()))
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_arguments_do_not_panic() {
        use std::os::unix::ffi::OsStringExt;
        let bad = OsString::from_vec(vec![0x66, 0x6f, 0x80]);
        assert!(matches!(parse([bad]), Err(CliError::UnknownArgument(_))));
    }
}
