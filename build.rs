use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn git_directory() -> Option<PathBuf> {
    let metadata = fs::metadata(".git").ok()?;
    if metadata.is_dir() {
        return Some(PathBuf::from(".git"));
    }
    let pointer = fs::read_to_string(".git").ok()?;
    pointer
        .trim()
        .strip_prefix("gitdir:")
        .map(str::trim)
        .map(PathBuf::from)
}

fn common_git_directory(git: &Path) -> PathBuf {
    fs::read_to_string(git.join("commondir"))
        .ok()
        .map(|value| git.join(value.trim()))
        .unwrap_or_else(|| git.to_owned())
}

fn valid_commit(value: &str) -> Option<String> {
    let value = value.trim();
    ((value.len() == 40 || value.len() == 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| value.to_ascii_lowercase())
}

fn packed_ref(common: &Path, reference: &str) -> Option<String> {
    let packed = common.join("packed-refs");
    println!("cargo:rerun-if-changed={}", packed.display());
    fs::read_to_string(packed).ok()?.lines().find_map(|line| {
        let (commit, name) = line.split_once(' ')?;
        (name == reference).then(|| valid_commit(commit)).flatten()
    })
}

fn repository_commit() -> Option<String> {
    let git = git_directory()?;
    let common = common_git_directory(&git);
    let head = git.join("HEAD");
    println!("cargo:rerun-if-changed={}", head.display());
    let value = fs::read_to_string(head).ok()?;
    let value = value.trim();
    let Some(reference) = value.strip_prefix("ref:").map(str::trim) else {
        return valid_commit(value);
    };
    for base in [&git, &common] {
        let path = base.join(reference);
        println!("cargo:rerun-if-changed={}", path.display());
        if let Some(commit) = fs::read_to_string(path)
            .ok()
            .and_then(|value| valid_commit(&value))
        {
            return Some(commit);
        }
    }
    packed_ref(&common, reference)
}

fn main() {
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
    println!("cargo:rerun-if-env-changed=GITHUB_RUN_ID");

    let commit = env::var("GITHUB_SHA")
        .ok()
        .and_then(|value| valid_commit(&value))
        .or_else(repository_commit);
    let provenance = env::var("GITHUB_RUN_ID")
        .map(|run| format!("github-actions:{run}"))
        .unwrap_or_else(|_| "local-build".to_owned());
    let target = env::var("TARGET").unwrap_or_else(|_| "unknown".to_owned());
    let profile = env::var("PROFILE").unwrap_or_else(|_| "unknown".to_owned());
    println!(
        "cargo:rustc-env=XUVA_BUILD_COMMIT={}",
        commit.as_deref().unwrap_or("unknown")
    );
    println!("cargo:rustc-env=XUVA_BUILD_PROVENANCE={provenance}");
    println!("cargo:rustc-env=XUVA_BUILD_TARGET={target}");
    println!("cargo:rustc-env=XUVA_BUILD_PROFILE={profile}");
}
