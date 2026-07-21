use std::path::Path;
use std::process::Command;

/// Stamp the short git SHA of the checked-out commit into the binary as
/// `SEANCE_GIT_SHA`. Falls back to `unknown` when git is unavailable (for
/// example a source tarball built off a release archive), so `--version`
/// always has something to print.
fn main() {
    let sha = Command::new("git")
        .args(["rev-parse", "--short=7", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=SEANCE_GIT_SHA={sha}");

    // Re-stamp when the checked-out commit moves. Guard each path on
    // existence so a git-less build does not force a rebuild every time
    // (a missing rerun-if-changed target counts as perpetually dirty).
    let git = Path::new("../../.git");
    for rel in ["HEAD", "packed-refs"] {
        let path = git.join(rel);
        if path.exists() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
    if let Ok(head) = std::fs::read_to_string(git.join("HEAD"))
        && let Some(reference) = head.strip_prefix("ref: ")
    {
        let ref_path = git.join(reference.trim());
        if ref_path.exists() {
            println!("cargo:rerun-if-changed={}", ref_path.display());
        }
    }
}
