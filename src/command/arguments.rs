use std::ffi::OsString;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArgumentSemantic {
    Opaque,
    Pattern,
    Revision,
    Path,
    PathList,
    ExecutablePath,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PathArgument<'a> {
    Whole(&'a str),
    Prefixed { prefix: &'a str, path: &'a str },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ArgumentContract<'a> {
    pub(crate) semantic: ArgumentSemantic,
    pub(crate) path: Option<PathArgument<'a>>,
}

fn whole_path(value: &str, semantic: ArgumentSemantic) -> ArgumentContract<'_> {
    ArgumentContract {
        semantic,
        path: Some(PathArgument::Whole(value)),
    }
}

fn prefixed_path<'a>(prefix: &'a str, path: &'a str) -> ArgumentContract<'a> {
    ArgumentContract {
        semantic: ArgumentSemantic::Path,
        path: Some(PathArgument::Prefixed { prefix, path }),
    }
}

fn semantic(semantic: ArgumentSemantic) -> ArgumentContract<'static> {
    ArgumentContract {
        semantic,
        path: None,
    }
}

fn separate_path_flag(tool: &str, flag: &str) -> bool {
    match tool {
        "git" => matches!(flag, "-C" | "--git-dir" | "--work-tree"),
        "go" => matches!(flag, "-C" | "-modfile" | "-overlay" | "-o"),
        "cargo" | "rustc" => matches!(
            flag,
            "--manifest-path" | "--target-dir" | "--out-dir" | "-o"
        ),
        "rg" | "fd" => matches!(flag, "--ignore-file"),
        _ => false,
    }
}

fn attached_path<'a>(tool: &str, value: &'a str) -> Option<(&'a str, &'a str)> {
    let prefixes: &[&str] = match tool {
        "git" => &["--git-dir=", "--work-tree="],
        "go" => &["-modfile=", "-overlay=", "-o="],
        "cargo" | "rustc" => &["--manifest-path=", "--target-dir=", "--out-dir="],
        "rg" | "fd" => &["--ignore-file=", "--file="],
        _ => &[],
    };
    prefixes
        .iter()
        .find_map(|prefix| value.strip_prefix(prefix).map(|path| (*prefix, path)))
}

fn rg_option_takes_value(value: &str) -> bool {
    matches!(
        value,
        "-e" | "--regexp"
            | "-f"
            | "--file"
            | "-g"
            | "--glob"
            | "--iglob"
            | "--ignore-file"
            | "-t"
            | "--type"
            | "-T"
            | "--type-not"
            | "--type-add"
            | "--type-clear"
            | "-A"
            | "--after-context"
            | "-B"
            | "--before-context"
            | "-C"
            | "--context"
            | "--context-separator"
            | "-E"
            | "--encoding"
            | "-M"
            | "--max-columns"
            | "-m"
            | "--max-count"
            | "--max-depth"
            | "--max-filesize"
            | "--path-separator"
            | "-r"
            | "--replace"
            | "--sort"
            | "--sortr"
            | "--threads"
            | "--trim"
    )
}

fn rg_known_flag(value: &str) -> bool {
    rg_option_takes_value(value)
        || matches!(
            value,
            "--" | "--files"
                | "--type-list"
                | "--json"
                | "--count"
                | "-c"
                | "--count-matches"
                | "--files-with-matches"
                | "-l"
                | "--files-without-match"
                | "--no-filename"
                | "-I"
                | "--with-filename"
                | "-H"
                | "--line-number"
                | "-n"
                | "--no-line-number"
                | "-N"
                | "--column"
                | "--heading"
                | "--no-heading"
                | "--hidden"
                | "-."
                | "--no-hidden"
                | "--no-ignore"
                | "--no-ignore-vcs"
                | "--no-ignore-parent"
                | "--no-ignore-global"
                | "--follow"
                | "-L"
                | "--fixed-strings"
                | "-F"
                | "--ignore-case"
                | "-i"
                | "--case-sensitive"
                | "-s"
                | "--smart-case"
                | "-S"
                | "--multiline"
                | "-U"
                | "--multiline-dotall"
                | "--pcre2"
                | "-P"
                | "--word-regexp"
                | "-w"
                | "--line-regexp"
                | "-x"
                | "--invert-match"
                | "-v"
                | "--text"
                | "-a"
                | "--binary"
                | "--null"
                | "-0"
                | "--null-data"
                | "--only-matching"
                | "-o"
                | "--passthru"
                | "--quiet"
                | "-q"
                | "--stats"
                | "--one-file-system"
        )
}

fn rg_positional_semantic(arguments: &[OsString], target: usize) -> ArgumentSemantic {
    let mut index = 0;
    let mut positional = 0;
    let mut pattern_supplied = false;
    let mut path_only_mode = false;
    let mut end_of_options = false;
    let mut fail_closed = false;
    while index <= target {
        let Some(value) = arguments.get(index).and_then(|argument| argument.to_str()) else {
            return ArgumentSemantic::Opaque;
        };
        if !end_of_options && value == "--" {
            end_of_options = true;
            if index == target {
                return ArgumentSemantic::Opaque;
            }
            index += 1;
            continue;
        }
        if !end_of_options && value.starts_with('-') && value != "-" {
            if value == "--files" || value == "--type-list" {
                path_only_mode = true;
            }
            if value == "-e" || value == "--regexp" || value == "-f" || value == "--file" {
                pattern_supplied = true;
            }
            if value
                .strip_prefix("--regexp=")
                .or_else(|| {
                    value
                        .strip_prefix("-e")
                        .filter(|pattern| !pattern.is_empty())
                })
                .is_some()
            {
                pattern_supplied = true;
            }
            if let Some(path) = value.strip_prefix("--file=") {
                if index == target {
                    return prefixed_path("--file=", path).semantic;
                }
                pattern_supplied = true;
            }
            if rg_option_takes_value(value) {
                if index + 1 == target {
                    return if matches!(value, "-f" | "--file" | "--ignore-file") {
                        ArgumentSemantic::Path
                    } else if matches!(value, "-e" | "--regexp" | "-g" | "--glob" | "--iglob") {
                        ArgumentSemantic::Pattern
                    } else {
                        ArgumentSemantic::Opaque
                    };
                }
                index += 2;
                continue;
            }
            fail_closed |= !rg_known_flag(value)
                && !value.starts_with("--regexp=")
                && !value.starts_with("--file=")
                && !value.starts_with("--glob=")
                && !value.starts_with("--iglob=");
            if index == target {
                return ArgumentSemantic::Opaque;
            }
            index += 1;
            continue;
        }
        if index == target {
            if fail_closed {
                return ArgumentSemantic::Opaque;
            }
            if path_only_mode || pattern_supplied || positional > 0 {
                return ArgumentSemantic::PathList;
            }
            return ArgumentSemantic::Pattern;
        }
        positional += 1;
        index += 1;
    }
    ArgumentSemantic::Opaque
}

pub(crate) fn argument_contract<'a>(
    tool: &str,
    arguments: &'a [OsString],
    index: usize,
) -> ArgumentContract<'a> {
    let Some(value) = arguments.get(index).and_then(|argument| argument.to_str()) else {
        return semantic(ArgumentSemantic::Opaque);
    };
    if index > 0
        && arguments[index - 1]
            .to_str()
            .is_some_and(|flag| separate_path_flag(tool, flag))
    {
        return whole_path(value, ArgumentSemantic::Path);
    }
    if let Some((prefix, path)) = attached_path(tool, value) {
        return prefixed_path(prefix, path);
    }
    if let Some(path) = value.strip_prefix('@')
        && matches!(tool, "cargo" | "rustc" | "go")
    {
        return prefixed_path("@", path);
    }
    match tool {
        "git" if arguments[..index].iter().any(|argument| argument == "--") => {
            whole_path(value, ArgumentSemantic::PathList)
        }
        "git" if index > 0 => semantic(ArgumentSemantic::Revision),
        "read" if !value.starts_with('-') => whole_path(value, ArgumentSemantic::Path),
        "proxy" | "run" if index == 0 => whole_path(value, ArgumentSemantic::ExecutablePath),
        "rg" | "fd" => {
            let argument_semantic = rg_positional_semantic(arguments, index);
            if matches!(
                argument_semantic,
                ArgumentSemantic::Path | ArgumentSemantic::PathList
            ) {
                whole_path(value, argument_semantic)
            } else {
                semantic(argument_semantic)
            }
        }
        _ => semantic(ArgumentSemantic::Opaque),
    }
}

pub(crate) fn has_typed_wsl_path(arguments: &[OsString]) -> bool {
    let Some(program) = arguments.first().and_then(|argument| argument.to_str()) else {
        return false;
    };
    if program.starts_with('/') {
        return true;
    }
    arguments[1..].iter().enumerate().any(|(index, _)| {
        argument_contract(program, &arguments[1..], index)
            .path
            .is_some_and(|path| match path {
                PathArgument::Whole(value) | PathArgument::Prefixed { path: value, .. } => {
                    value.starts_with('/')
                }
            })
    })
}
