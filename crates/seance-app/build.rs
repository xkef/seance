//! Stamps the short git SHA into the binary as `SEANCE_GIT_SHA` so
//! `seance --version` can print `séance <version> (<sha>)`. Falls back to
//! `unknown` outside a git checkout (e.g. release tarball builds).

use std::path::Path;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn main() {
    let sha = git(&["rev-parse", "--short=7", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
    println!("cargo::rustc-env=SEANCE_GIT_SHA={sha}");

    // Re-stamp when HEAD moves: .git/HEAD changes on checkout/detach, and
    // the branch ref file it points at changes on commit.
    if let Some(git_dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        let head = Path::new(&git_dir).join("HEAD");
        if head.exists() {
            println!("cargo::rerun-if-changed={}", head.display());
        }
        if let Some(ref_name) = git(&["symbolic-ref", "-q", "HEAD"]) {
            let ref_file = Path::new(&git_dir).join(ref_name);
            if ref_file.exists() {
                println!("cargo::rerun-if-changed={}", ref_file.display());
            }
        }
    }
}
