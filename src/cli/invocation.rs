use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

use crate::PRODUCT_COMMAND;
use crate::cli::parse_options;
use crate::cli_exit::CliExit as ExitCode;
use crate::command::{ClassifiedCommand, classify};
use crate::config::{Config, ExecutionEnvironment, Route};
use crate::execution::planner::is_shell_operator_command;

#[derive(Clone, Debug)]
pub(crate) struct InvocationRequest {
    pub(crate) arguments: Vec<OsString>,
    pub(crate) requested_route: Route,
    pub(crate) environment: ExecutionEnvironment,
    pub(crate) explain: bool,
    pub(crate) config: Config,
    pub(crate) current_directory: Option<PathBuf>,
    pub(crate) command: ClassifiedCommand,
}

#[derive(Clone, Debug)]
pub(crate) struct InvocationError {
    pub(crate) message: String,
    pub(crate) exit: ExitCode,
}

pub(crate) fn parse(
    arguments: Vec<OsString>,
    config: &Config,
) -> Result<InvocationRequest, InvocationError> {
    let (arguments, requested_route, environment, explain) =
        parse_options(arguments, config.route_preference, config.environment).map_err(
            |message| InvocationError {
                message,
                exit: ExitCode::FAILURE,
            },
        )?;
    if arguments.is_empty() {
        return Err(InvocationError {
            message: format!(
                "{PRODUCT_COMMAND}: no command supplied; run `{PRODUCT_COMMAND} --help` for usage"
            ),
            exit: ExitCode::FAILURE,
        });
    }
    if is_shell_operator_command(&arguments) {
        return Err(InvocationError {
            message: format!(
                "xuva: `{}` is shell syntax, not an executable; let PowerShell, cmd, or a POSIX shell own the pipeline and invoke XUVA only for command argv",
                arguments[0].to_string_lossy()
            ),
            exit: ExitCode::from(2),
        });
    }
    let mut invocation_config = config.clone();
    invocation_config.environment = environment;
    let command = classify(&arguments);
    Ok(InvocationRequest {
        arguments,
        requested_route,
        environment,
        explain,
        config: invocation_config,
        current_directory: env::current_dir().ok(),
        command,
    })
}

impl InvocationRequest {
    pub(crate) fn current_directory_str(&self) -> Option<&str> {
        self.current_directory
            .as_deref()
            .and_then(|path| path.to_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_returns_an_owned_typed_invocation() {
        let config = Config::from_lookup(|_| None).expect("default config is valid");
        let request = parse(
            vec![
                OsString::from("--route"),
                OsString::from("raw"),
                OsString::from("git"),
                OsString::from("status"),
            ],
            &config,
        )
        .expect("invocation parses");
        assert_eq!(request.requested_route, Route::Raw);
        assert_eq!(request.command.family(&request.arguments), "git");
        assert_eq!(
            request
                .command
                .git
                .as_ref()
                .and_then(|git| git.subcommand(&request.arguments)),
            Some("status")
        );
    }

    #[test]
    fn parse_rejects_shell_operators_before_planning() {
        let config = Config::from_lookup(|_| None).expect("default config is valid");
        let error = parse(vec![OsString::from("|")], &config).expect_err("operator fails");
        assert_eq!(error.exit, ExitCode::from(2));
    }
}
