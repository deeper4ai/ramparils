//! Build script: bake the source revision into the binary.
//!
//! `CARGO_PKG_VERSION` only moves on release, so it spans every commit since
//! the last tag — a log line saying `v0.1.3` does not say which code ran. The
//! git revision does, and a run's log is the only record that survives it.
//!
//! Everything here degrades to `unknown` rather than failing: builds from an
//! sdist (maturin, pip) or a source tarball have no `.git` at all, and a build
//! must not depend on git being installed.

use std::process::Command;

fn run(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn main() {
    let revision = run("git", &["rev-parse", "--short", "HEAD"]).map(|sha| {
        // A dirty worktree means the revision does not identify what was
        // built, which is exactly what a reader of the log needs to know.
        // Untracked files are excluded deliberately: scratch directories and
        // stray logs do not change what gets compiled, and counting them would
        // stamp `-dirty` on every build a working checkout ever makes, which
        // trains the reader to ignore the flag.
        let dirty = run("git", &["status", "--porcelain", "--untracked-files=no"]).is_some();
        if dirty { format!("{sha}-dirty") } else { sha }
    });
    println!(
        "cargo:rustc-env=RAMPARILS_GIT={}",
        revision.unwrap_or_else(|| "unknown".to_string())
    );

    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let rustc_version = run(&rustc, &["--version"]).unwrap_or_else(|| "rustc unknown".to_string());
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "unknown".to_string());
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string());
    println!("cargo:rustc-env=RAMPARILS_BUILD={profile}, {rustc_version}, {target}");

    // Without these the build script is cached and the revision goes stale:
    // commit, rebuild without touching a source file, and the binary keeps
    // reporting the previous commit — worse than reporting nothing, because it
    // is confidently wrong. When `.git` is absent these paths do not exist,
    // which makes cargo rerun the script on every build; it is cheap enough.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
}
