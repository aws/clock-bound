//! Build script for clock-bound-ff-tester

use std::io::ErrorKind;
use std::process::Command;

fn git_exists() -> bool {
    match Command::new("git").arg("--version").output() {
        Ok(_) => true,
        Err(e) if e.kind() == ErrorKind::NotFound => false,
        Err(_) => false, // Other error treat as unusable, could log error if needed
    }
}

/// Shell out and run a git command
fn run_git(args: &[&str]) -> String {
    let output = Command::new("git").args(args).output().unwrap();
    String::from_utf8(output.stdout).unwrap()
}

/// Return the short hash of the repo latest commit
fn git_commit_hash_short() -> String {
    run_git(&["rev-parse", "--short", "HEAD"])
}

/// Detect whether the repository is clean or dirty
fn git_repo_status() -> String {
    let output = run_git(&["status", "--porcelain", "--untracked-files=no"]);
    if output.trim().is_empty() {
        String::from("clean")
    } else {
        String::from("dirty")
    }
}

fn main() {
    // If building from a git repository, emit some extra information to tag the version of the
    // ffsync algorithm being used.
    if git_exists() {
        let short_hash = git_commit_hash_short();
        let status = git_repo_status();

        println!("cargo:rustc-env=GIT_COMMIT_HASH_SHORT={short_hash}");
        println!("cargo:rustc-env=GIT_REPO_STATUS={status}");
    }
}
