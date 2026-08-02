use std::ffi::OsString;

#[derive(Clone, Copy, PartialEq, Eq)]
enum PatternShape {
    Literal,
    Alternation,
    Complex,
    Mixed,
}

impl PatternShape {
    fn as_str(self) -> &'static str {
        match self {
            Self::Literal => "lit",
            Self::Alternation => "alt",
            Self::Complex => "complex",
            Self::Mixed => "mix",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PatternSize {
    Small,
    Medium,
    Large,
    Mixed,
}

impl PatternSize {
    fn as_str(self) -> &'static str {
        match self {
            Self::Small => "s",
            Self::Medium => "m",
            Self::Large => "l",
            Self::Mixed => "mix",
        }
    }
}

fn merge<T: Copy + PartialEq>(current: &mut Option<T>, next: T, mixed: T) {
    *current = Some(match *current {
        None => next,
        Some(existing) if existing == next => existing,
        Some(_) => mixed,
    });
}

fn observe_pattern(
    value: &OsString,
    shape: &mut Option<PatternShape>,
    size: &mut Option<PatternSize>,
    count: &mut usize,
) -> Option<()> {
    let value = value.to_str()?;
    let mut escaped = false;
    let mut alternation = false;
    let mut complex = false;
    for byte in value.bytes() {
        if escaped {
            escaped = false;
            complex = true;
            continue;
        }
        match byte {
            b'\\' => escaped = true,
            b'|' => alternation = true,
            b'.' | b'*' | b'+' | b'?' | b'(' | b')' | b'[' | b']' | b'{' | b'}' | b'^' | b'$' => {
                complex = true
            }
            _ => {}
        }
    }
    let next_shape = if alternation {
        PatternShape::Alternation
    } else if complex || escaped {
        PatternShape::Complex
    } else {
        PatternShape::Literal
    };
    let next_size = match value.len() {
        0..=16 => PatternSize::Small,
        17..=64 => PatternSize::Medium,
        _ => PatternSize::Large,
    };
    merge(shape, next_shape, PatternShape::Mixed);
    merge(size, next_size, PatternSize::Mixed);
    *count += 1;
    Some(())
}

fn count_bucket(count: usize) -> &'static str {
    match count {
        0 => "none",
        1 => "one",
        _ => "many",
    }
}

fn set_mode(current: &mut &'static str, next: &'static str) -> Option<()> {
    if *current == "default" || *current == next {
        *current = next;
        Some(())
    } else {
        None
    }
}

fn separate_value_option(value: &str) -> bool {
    matches!(
        value,
        "-A" | "-B"
            | "-C"
            | "-E"
            | "-M"
            | "-e"
            | "-f"
            | "-g"
            | "-j"
            | "-m"
            | "-r"
            | "-t"
            | "-T"
            | "--after-context"
            | "--before-context"
            | "--color"
            | "--colors"
            | "--context"
            | "--context-separator"
            | "--dfa-size-limit"
            | "--encoding"
            | "--engine"
            | "--field-context-separator"
            | "--field-match-separator"
            | "--file"
            | "--glob"
            | "--hostname-bin"
            | "--hyperlink-format"
            | "--iglob"
            | "--ignore-file"
            | "--max-columns"
            | "--max-count"
            | "--max-depth"
            | "--max-filesize"
            | "--path-separator"
            | "--pre"
            | "--pre-glob"
            | "--regex-size-limit"
            | "--regexp"
            | "--replace"
            | "--sort"
            | "--sortr"
            | "--threads"
            | "--type"
            | "--type-add"
            | "--type-clear"
            | "--type-not"
    )
}

fn known_flag(value: &str) -> bool {
    matches!(
        value,
        "-0" | "-F"
            | "-H"
            | "-I"
            | "-L"
            | "-N"
            | "-P"
            | "-S"
            | "-U"
            | "-V"
            | "-a"
            | "-b"
            | "-c"
            | "-h"
            | "-i"
            | "-l"
            | "-n"
            | "-o"
            | "-p"
            | "-q"
            | "-s"
            | "-u"
            | "-v"
            | "-w"
            | "-x"
            | "--binary"
            | "--block-buffered"
            | "--byte-offset"
            | "--case-sensitive"
            | "--column"
            | "--count"
            | "--count-matches"
            | "--crlf"
            | "--debug"
            | "--files"
            | "--files-with-matches"
            | "--files-without-match"
            | "--fixed-strings"
            | "--follow"
            | "--heading"
            | "--hidden"
            | "--ignore-case"
            | "--include-zero"
            | "--invert-match"
            | "--json"
            | "--line-buffered"
            | "--line-number"
            | "--mmap"
            | "--multiline"
            | "--multiline-dotall"
            | "--no-config"
            | "--no-filename"
            | "--no-heading"
            | "--no-ignore"
            | "--no-ignore-dot"
            | "--no-ignore-exclude"
            | "--no-ignore-files"
            | "--no-ignore-global"
            | "--no-ignore-messages"
            | "--no-ignore-parent"
            | "--no-ignore-vcs"
            | "--no-line-number"
            | "--no-mmap"
            | "--no-messages"
            | "--no-pcre2-unicode"
            | "--no-require-git"
            | "--no-unicode"
            | "--null"
            | "--null-data"
            | "--one-file-system"
            | "--only-matching"
            | "--passthru"
            | "--pcre2"
            | "--pcre2-unicode"
            | "--pretty"
            | "--quiet"
            | "--smart-case"
            | "--stats"
            | "--stop-on-nonmatch"
            | "--text"
            | "--trim"
            | "--type-list"
            | "--unrestricted"
            | "--version"
            | "--vimgrep"
            | "--with-filename"
            | "--word-regexp"
            | "--line-regexp"
    )
}

fn attached_value(value: &str) -> Option<(&str, &str)> {
    const OPTIONS: &[&str] = &[
        "--after-context=",
        "--before-context=",
        "--color=",
        "--colors=",
        "--context=",
        "--dfa-size-limit=",
        "--encoding=",
        "--engine=",
        "--file=",
        "--glob=",
        "--hostname-bin=",
        "--hyperlink-format=",
        "--iglob=",
        "--ignore-file=",
        "--max-columns=",
        "--max-count=",
        "--max-depth=",
        "--max-filesize=",
        "--pre=",
        "--pre-glob=",
        "--regex-size-limit=",
        "--regexp=",
        "--replace=",
        "--sort=",
        "--sortr=",
        "--threads=",
        "--type=",
        "--type-add=",
        "--type-clear=",
        "--type-not=",
    ];
    OPTIONS
        .iter()
        .find_map(|prefix| value.strip_prefix(prefix).map(|rest| (*prefix, rest)))
}

pub(crate) fn rg_workload_key(arguments: &[OsString]) -> Option<String> {
    if arguments.first()?.to_str()? != "rg" {
        return None;
    }
    let mut pattern_source = "pos";
    let mut pattern_shape = None;
    let mut pattern_size = None;
    let mut pattern_count = 0;
    let mut path_count = 0;
    let mut filter_count = 0;
    let mut output = "default";
    let mut matcher = "default";
    let mut case = "default";
    let mut layout = "default";
    let mut positional_pattern_seen = false;
    let mut after_separator = false;
    let mut index = 1;

    while let Some(argument) = arguments.get(index) {
        let value = argument.to_str()?;
        if !after_separator && value == "--" {
            after_separator = true;
            index += 1;
            continue;
        }
        if !after_separator && value.starts_with('-') && value != "-" {
            if let Some((prefix, attached)) = attached_value(value) {
                if prefix == "--regexp=" {
                    pattern_source = if pattern_count == 0 { "re" } else { "mix" };
                    observe_pattern(
                        &OsString::from(attached),
                        &mut pattern_shape,
                        &mut pattern_size,
                        &mut pattern_count,
                    )?;
                } else if prefix == "--file=" {
                    pattern_source = if pattern_count == 0 { "file" } else { "mix" };
                    pattern_count += 1;
                } else if matches!(
                    prefix,
                    "--glob="
                        | "--iglob="
                        | "--ignore-file="
                        | "--type="
                        | "--type-add="
                        | "--type-clear="
                        | "--type-not="
                ) {
                    filter_count += 1;
                } else if matches!(
                    prefix,
                    "--after-context=" | "--before-context=" | "--context="
                ) {
                    set_mode(&mut layout, "context")?;
                }
                index += 1;
                continue;
            }
            if value.len() > 2 && matches!(&value[..2], "-e" | "-f" | "-g" | "-t" | "-T") {
                match &value[..2] {
                    "-e" => {
                        pattern_source = if pattern_count == 0 { "re" } else { "mix" };
                        observe_pattern(
                            &OsString::from(&value[2..]),
                            &mut pattern_shape,
                            &mut pattern_size,
                            &mut pattern_count,
                        )?;
                    }
                    "-f" => {
                        pattern_source = if pattern_count == 0 { "file" } else { "mix" };
                        pattern_count += 1;
                    }
                    "-g" | "-t" | "-T" => filter_count += 1,
                    _ => unreachable!(),
                }
                index += 1;
                continue;
            }
            if separate_value_option(value) {
                let next = arguments.get(index + 1)?;
                match value {
                    "-e" | "--regexp" => {
                        pattern_source = if pattern_count == 0 { "re" } else { "mix" };
                        observe_pattern(
                            next,
                            &mut pattern_shape,
                            &mut pattern_size,
                            &mut pattern_count,
                        )?;
                    }
                    "-f" | "--file" => {
                        pattern_source = if pattern_count == 0 { "file" } else { "mix" };
                        pattern_count += 1;
                    }
                    "-g" | "-t" | "-T" | "--glob" | "--iglob" | "--ignore-file" | "--type"
                    | "--type-add" | "--type-clear" | "--type-not" => filter_count += 1,
                    "-A" | "-B" | "-C" | "--after-context" | "--before-context" | "--context" => {
                        set_mode(&mut layout, "context")?
                    }
                    "--engine" => matcher = "engine",
                    _ => {}
                }
                index += 2;
                continue;
            }
            if !known_flag(value) {
                return None;
            }
            match value {
                "-c" | "--count" | "--count-matches" => set_mode(&mut output, "count")?,
                "-l"
                | "--files-with-matches"
                | "--files-without-match"
                | "--files"
                | "--type-list" => set_mode(&mut output, "files")?,
                "--json" => set_mode(&mut output, "json")?,
                "-q" | "--quiet" => set_mode(&mut output, "quiet")?,
                "--stats" => set_mode(&mut output, "stats")?,
                "-F" | "--fixed-strings" => set_mode(&mut matcher, "fixed")?,
                "-P" | "--pcre2" => set_mode(&mut matcher, "pcre2")?,
                "-i" | "--ignore-case" => set_mode(&mut case, "insensitive")?,
                "-s" | "--case-sensitive" => set_mode(&mut case, "sensitive")?,
                "-S" | "--smart-case" => set_mode(&mut case, "smart")?,
                "-o" | "--only-matching" => set_mode(&mut layout, "only")?,
                "-v" | "--invert-match" => set_mode(&mut layout, "invert")?,
                _ => {}
            }
            index += 1;
            continue;
        }

        if (output == "files" && pattern_count == 0) || pattern_count > 0 || positional_pattern_seen
        {
            path_count += 1;
        } else {
            observe_pattern(
                argument,
                &mut pattern_shape,
                &mut pattern_size,
                &mut pattern_count,
            )?;
            positional_pattern_seen = true;
        }
        index += 1;
    }

    if pattern_count == 0 && output != "files" {
        return None;
    }
    let shape = pattern_shape
        .map(PatternShape::as_str)
        .unwrap_or("external");
    let size = pattern_size.map(PatternSize::as_str).unwrap_or("external");
    let output = if output == "default" { "lines" } else { output };
    let matcher = if matcher == "default" {
        "regex"
    } else {
        matcher
    };
    let case = match case {
        "default" => "d",
        "insensitive" => "i",
        "sensitive" => "s",
        "smart" => "smart",
        value => value,
    };
    let layout = match layout {
        "default" => "full",
        "context" => "ctx",
        value => value,
    };
    Some(format!(
        "rg:v1:s-{pattern_source}:p-{shape}-{size}-{}:r-{}:o-{output}:m-{matcher}:c-{case}:l-{layout}:f-{}",
        count_bucket(pattern_count),
        count_bucket(path_count),
        count_bucket(filter_count),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn rg_keys_encode_shape_without_pattern_or_path_content() {
        let focused = rg_workload_key(&args(&["rg", "RegexBuilder", "crates", "tests"]))
            .expect("focused search is classified");
        let renamed = rg_workload_key(&args(&["rg", "AnotherName", "src", "fixtures"]))
            .expect("equivalent shape is classified");
        let broad = rg_workload_key(&args(&["rg", "fn|struct|impl|use|pub", "crates", "tests"]))
            .expect("alternation search is classified");

        assert_eq!(focused, renamed);
        assert_ne!(focused, broad);
        for secret in ["RegexBuilder", "AnotherName", "crates", "tests"] {
            assert!(!focused.contains(secret));
        }
    }

    #[test]
    fn rg_keys_are_exact_for_output_matcher_and_operand_shape() {
        let base =
            rg_workload_key(&args(&["rg", "needle", "src"])).expect("base search is classified");
        let json = rg_workload_key(&args(&["rg", "--json", "needle", "src"]))
            .expect("json search is classified");
        let fixed = rg_workload_key(&args(&["rg", "-F", "needle", "src"]))
            .expect("fixed search is classified");
        let many_paths = rg_workload_key(&args(&["rg", "needle", "src", "tests"]))
            .expect("multi-root search is classified");
        assert_ne!(base, json);
        assert_ne!(base, fixed);
        assert_ne!(base, many_paths);
        for key in [base, json, fixed, many_paths] {
            assert!(key.len() <= 128);
        }
    }

    #[test]
    fn unknown_or_incomplete_rg_forms_fail_closed() {
        assert!(rg_workload_key(&args(&["rg", "--future-option", "needle"])).is_none());
        assert!(rg_workload_key(&args(&["rg", "--regexp"])).is_none());
        assert!(rg_workload_key(&args(&["rg"])).is_none());
    }
}
