use std::ffi::OsString;

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
}
