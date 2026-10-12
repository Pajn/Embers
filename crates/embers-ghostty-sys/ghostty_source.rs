//! The Ghostty source libghostty-vt is built from, shared by this crate's build
//! script and by `embers-ghostty-gen`, which generates the bindings from the
//! same commit's headers.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The Ghostty commit the library is built from and the bindings are
/// generated from. Change it only together with regenerating the bindings
/// (`cargo gen-ghostty`).
pub const GHOSTTY_COMMIT: &str = "9f6a4640e6bca7a4d65f1b441f2666b151f4d402";

/// The Zig release that commit builds with.
pub const ZIG_VERSION: &str = "0.16.0";

const GHOSTTY_REPOSITORY: &str = "https://github.com/ghostty-org/ghostty.git";

/// Names a Ghostty checkout to use instead of fetching [`GHOSTTY_COMMIT`].
pub const SOURCE_DIR_ENV: &str = "GHOSTTY_SOURCE_DIR";

/// The Ghostty source: `GHOSTTY_SOURCE_DIR` when set, else [`GHOSTTY_COMMIT`]
/// fetched into `cache_dir`. The fetch is by commit hash, which also checks
/// that the content is the commit's.
pub fn ghostty_source(cache_dir: &Path) -> Result<PathBuf, String> {
    if let Some(dir) = std::env::var_os(SOURCE_DIR_ENV) {
        let dir = PathBuf::from(dir);
        if !dir.join("build.zig").exists() {
            return Err(format!(
                "{SOURCE_DIR_ENV} is {}, which has no build.zig",
                dir.display()
            ));
        }
        return Ok(dir);
    }

    let dir = cache_dir.join(format!("ghostty-{GHOSTTY_COMMIT}"));
    if is_checkout_of_pin(&dir) {
        return Ok(dir);
    }
    // Anything else there is not a usable checkout, such as one a build cache
    // restored only in part, and would keep the fetched one from moving in.
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .map_err(|error| format!("failed to remove {}: {error}", dir.display()))?;
    }

    // Fetched beside the final directory and moved into place once complete,
    // so an interrupted fetch is never taken for a checkout.
    let partial = cache_dir.join(format!("ghostty-{GHOSTTY_COMMIT}.partial"));
    if partial.exists() {
        std::fs::remove_dir_all(&partial)
            .map_err(|error| format!("failed to remove {}: {error}", partial.display()))?;
    }
    std::fs::create_dir_all(&partial)
        .map_err(|error| format!("failed to create {}: {error}", partial.display()))?;
    git(&partial, &["init", "--quiet"])?;
    git(
        &partial,
        &[
            "fetch",
            "--quiet",
            "--depth=1",
            GHOSTTY_REPOSITORY,
            GHOSTTY_COMMIT,
        ],
    )?;
    git(&partial, &["checkout", "--quiet", "FETCH_HEAD"])?;
    std::fs::rename(&partial, &dir)
        .map_err(|error| format!("failed to move the Ghostty checkout into place: {error}"))?;
    Ok(dir)
}

/// Whether `dir` is a checkout of [`GHOSTTY_COMMIT`] with every tracked file
/// as committed. Untracked files, such as Zig's build cache, do not count.
fn is_checkout_of_pin(dir: &Path) -> bool {
    if !dir.join("build.zig").exists() {
        return false;
    }
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output();
    let at_pin = head.is_ok_and(|output| {
        output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == GHOSTTY_COMMIT
    });
    at_pin
        && Command::new("git")
            .args(["diff", "--quiet", "HEAD"])
            .current_dir(dir)
            .status()
            .is_ok_and(|status| status.success())
}

fn git(dir: &Path, args: &[&str]) -> Result<(), String> {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .map_err(|error| format!("could not run git ({error}); git is needed to fetch Ghostty"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "`git {}` failed in {}",
            args.join(" "),
            dir.display()
        ))
    }
}
