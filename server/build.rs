//! Build script: capture the git short hash for the `/api/v1/version` endpoint.
//!
//! Resolution order:
//! 1. `GIT_HASH` build-time env var — set by the Docker build via `--build-arg`,
//!    because the Docker build context does not include the `.git` directory.
//! 2. `git rev-parse --short HEAD` — for local / `cargo run` builds.
//! 3. `"unknown"` — if neither is available.

use std::process::Command;

fn main() {
    let hash = std::env::var("GIT_HASH")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| run_git(&["rev-parse", "--short", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=GIT_HASH={hash}");

    // Rebuild when the injected hash changes (Docker path)...
    println!("cargo:rerun-if-env-changed=GIT_HASH");
    // ...and when HEAD moves (local path). `--git-dir` resolves correctly in
    // linked worktrees, where `.git` is a file pointing at the real gitdir.
    if let Some(git_dir) = run_git(&["rev-parse", "--git-dir"]) {
        println!("cargo:rerun-if-changed={git_dir}/HEAD");
    }
}

/// Run a git command, returning trimmed stdout on success, `None` otherwise.
fn run_git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!text.is_empty()).then_some(text)
}
