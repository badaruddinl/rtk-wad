use std::ffi::OsString;

mod arguments;

#[cfg(test)]
pub(crate) use arguments::ArgumentSemantic;
pub(crate) use arguments::{PathArgument, argument_contract, has_typed_wsl_path};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CommandAccess {
    ReadOnly,
    MutatingOrUnknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GitCommand {
    subcommand_index: Option<usize>,
    pub(crate) access: CommandAccess,
    pub(crate) uses_wsl_directory: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ClassifiedCommand {
    pub(crate) git: Option<GitCommand>,
}

pub(crate) fn classify(arguments: &[OsString]) -> ClassifiedCommand {
    let is_git = arguments.first().and_then(|argument| argument.to_str()) == Some("git");
    ClassifiedCommand {
        git: is_git.then(|| classify_git(arguments)),
    }
}

fn classify_git(arguments: &[OsString]) -> GitCommand {
    if matches!(
        arguments,
        [program, option]
            if program == "git"
                && matches!(option.to_str(), Some("--version" | "-v" | "--help" | "-h"))
    ) {
        return GitCommand {
            subcommand_index: None,
            access: CommandAccess::ReadOnly,
            uses_wsl_directory: false,
        };
    }

    let mut index = 1;
    let mut uses_wsl_directory = false;
    while let Some(argument) = arguments.get(index) {
        let Some(value) = argument.to_str() else {
            return unknown_git(uses_wsl_directory);
        };
        if value == "--" {
            return unknown_git(uses_wsl_directory);
        }
        if is_global_option_with_separate_value(value) {
            let Some(option_value) = arguments.get(index + 1) else {
                return unknown_git(uses_wsl_directory);
            };
            if is_git_directory_option(value) && is_wsl_path(option_value) {
                uses_wsl_directory = true;
            }
            index += 2;
            continue;
        }
        if let Some(path) = attached_git_directory(value) {
            uses_wsl_directory |= path.starts_with('/');
            index += 1;
            continue;
        }
        if is_known_global_flag(value) || is_attached_global_value(value) {
            index += 1;
            continue;
        }
        if value.starts_with('-') {
            return unknown_git(uses_wsl_directory);
        }
        let access = if is_read_only_git_subcommand(value) {
            CommandAccess::ReadOnly
        } else {
            CommandAccess::MutatingOrUnknown
        };
        return GitCommand {
            subcommand_index: Some(index),
            access,
            uses_wsl_directory,
        };
    }
    unknown_git(uses_wsl_directory)
}

fn unknown_git(uses_wsl_directory: bool) -> GitCommand {
    GitCommand {
        subcommand_index: None,
        access: CommandAccess::MutatingOrUnknown,
        uses_wsl_directory,
    }
}

fn is_global_option_with_separate_value(value: &str) -> bool {
    matches!(
        value,
        "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace"
    )
}

fn is_git_directory_option(value: &str) -> bool {
    matches!(value, "-C" | "--git-dir" | "--work-tree")
}

fn attached_git_directory(value: &str) -> Option<&str> {
    value
        .strip_prefix("--git-dir=")
        .or_else(|| value.strip_prefix("--work-tree="))
        .or_else(|| value.strip_prefix("-C").filter(|path| !path.is_empty()))
}

fn is_known_global_flag(value: &str) -> bool {
    matches!(
        value,
        "--paginate"
            | "-p"
            | "--no-pager"
            | "-P"
            | "--bare"
            | "--no-replace-objects"
            | "--literal-pathspecs"
            | "--glob-pathspecs"
            | "--noglob-pathspecs"
            | "--icase-pathspecs"
            | "--no-optional-locks"
            | "--no-lazy-fetch"
    )
}

fn is_attached_global_value(value: &str) -> bool {
    value.starts_with("--namespace=")
        || value.starts_with("--super-prefix=")
        || value.starts_with("--config-env=")
        || value
            .strip_prefix("-c")
            .is_some_and(|setting| !setting.is_empty())
}

fn is_read_only_git_subcommand(value: &str) -> bool {
    matches!(
        value,
        "status" | "log" | "show" | "diff" | "rev-parse" | "ls-files" | "grep"
    )
}

fn is_wsl_path(value: &OsString) -> bool {
    value.to_string_lossy().starts_with('/')
}

impl ClassifiedCommand {
    pub(crate) fn family<'a>(&self, arguments: &'a [OsString]) -> &'a str {
        arguments
            .first()
            .and_then(|argument| argument.to_str())
            .unwrap_or("unknown")
    }

    pub(crate) fn metric_family(&self, arguments: &[OsString]) -> String {
        let executable = arguments
            .first()
            .map(|argument| argument.to_string_lossy())
            .unwrap_or_default();
        let basename = executable.rsplit(['/', '\\']).next().unwrap_or_default();
        let basename = [".exe", ".cmd", ".bat", ".com"]
            .iter()
            .find_map(|suffix| {
                basename
                    .get(basename.len().saturating_sub(suffix.len())..)
                    .is_some_and(|ending| ending.eq_ignore_ascii_case(suffix))
                    .then(|| &basename[..basename.len() - suffix.len()])
            })
            .unwrap_or(basename);
        let mut family = if !basename.is_empty()
            && basename.len() <= 48
            && basename
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            basename.to_ascii_lowercase()
        } else {
            "unknown".to_owned()
        };
        if family == "git"
            && let Some(subcommand) = self
                .git
                .as_ref()
                .and_then(|git| git.subcommand(arguments))
                .filter(|subcommand| is_read_only_git_subcommand(subcommand))
        {
            family.push(':');
            family.push_str(subcommand);
        }
        family
    }
}

impl GitCommand {
    pub(crate) fn subcommand<'a>(&self, arguments: &'a [OsString]) -> Option<&'a str> {
        arguments
            .get(self.subcommand_index?)
            .and_then(|argument| argument.to_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn git_global_options_are_parsed_before_the_subcommand() {
        let arguments = args(&[
            "git",
            "-c",
            "alias.status=commit",
            "-C",
            "/mnt/e/work",
            "status",
            "commit",
        ]);
        let classified = classify(&arguments);
        let git = classified.git.expect("Git is classified");
        assert_eq!(git.subcommand(&arguments), Some("status"));
        assert_eq!(git.access, CommandAccess::ReadOnly);
        assert!(git.uses_wsl_directory);
    }

    #[test]
    fn attached_git_directory_options_are_typed_paths() {
        for option in ["-C/mnt/e/work", "--git-dir=/tmp/repo", "--work-tree=/src"] {
            let git = classify(&args(&["git", option, "status"]))
                .git
                .expect("Git is classified");
            assert!(git.uses_wsl_directory, "{option}");
            assert_eq!(git.access, CommandAccess::ReadOnly);
        }
    }

    #[test]
    fn unknown_global_options_fail_closed() {
        let arguments = args(&["git", "--future-option", "status"]);
        let git = classify(&arguments).git.expect("Git is classified");
        assert_eq!(git.subcommand(&arguments), None);
        assert_eq!(git.access, CommandAccess::MutatingOrUnknown);
    }

    #[test]
    fn mutation_and_read_only_arguments_are_not_confused() {
        let mutation = classify(&args(&["git", "commit", "status"]))
            .git
            .expect("Git is classified");
        assert_eq!(mutation.access, CommandAccess::MutatingOrUnknown);

        let read_only = classify(&args(&["git", "status", "commit"]))
            .git
            .expect("Git is classified");
        assert_eq!(read_only.access, CommandAccess::ReadOnly);
    }

    #[test]
    fn data_that_looks_like_a_linux_path_is_not_a_typed_path() {
        let rg = args(&["rg", "/api/", "src"]);
        assert_eq!(
            argument_contract("rg", &rg[1..], 0).semantic,
            ArgumentSemantic::Pattern
        );
        assert_eq!(
            argument_contract("rg", &rg[1..], 1).semantic,
            ArgumentSemantic::PathList
        );
        assert!(!has_typed_wsl_path(&rg));

        let git = args(&["git", "show", "/release/"]);
        assert_eq!(
            argument_contract("git", &git[1..], 1).semantic,
            ArgumentSemantic::Revision
        );
        assert!(!has_typed_wsl_path(&git));
    }

    #[test]
    fn only_explicit_path_positions_affect_wsl_routing() {
        assert!(has_typed_wsl_path(&args(&["rg", "pattern", "/mnt/c/src"])));
        assert!(has_typed_wsl_path(&args(&[
            "git",
            "status",
            "--",
            "/mnt/c/src/file.rs"
        ])));
        assert!(has_typed_wsl_path(&args(&[
            "git",
            "-C",
            "/tmp/repo",
            "status"
        ])));
        assert!(has_typed_wsl_path(&args(&["/bin/sh", "-c", "true"])));
    }

    #[test]
    fn metric_family_is_bounded_to_basename_and_allowlisted_subcommands() {
        let explicit = args(&[r"C:\\Users\\alice\\Tools\\RG.EXE", "secret-pattern"]);
        assert_eq!(classify(&explicit).metric_family(&explicit), "rg");

        let git_status = args(&["git", "status", "--short"]);
        assert_eq!(
            classify(&git_status).metric_family(&git_status),
            "git:status"
        );

        let git_unknown = args(&["git", "credential-secret"]);
        assert_eq!(classify(&git_unknown).metric_family(&git_unknown), "git");

        let unsafe_name = args(&[r"C:\\Users\\alice\\tool secret.exe"]);
        assert_eq!(
            classify(&unsafe_name).metric_family(&unsafe_name),
            "unknown"
        );
    }
}
