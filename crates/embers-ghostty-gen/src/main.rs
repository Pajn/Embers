//! Regenerates `embers-ghostty-sys`'s checked-in bindings from the headers of
//! the Ghostty commit that crate pins.
//!
//! This lives in its own crate rather than as a bin of `embers-ghostty-sys`,
//! so it builds without Zig and still builds when stale bindings have left the
//! sys crate failing to compile.

#[allow(dead_code)]
#[path = "../../embers-ghostty-sys/ghostty_source.rs"]
mod ghostty_source;

use std::path::PathBuf;
use std::process::{Command, ExitCode};

use ghostty_source::{GHOSTTY_COMMIT, ghostty_source};

/// The `bindgen` release the checked-in bindings are generated with. Keep it in
/// step with `BINDGEN_VERSION` in CI.
const BINDGEN_VERSION: &str = "0.72.1";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let sys_dir = manifest_dir.join("../embers-ghostty-sys");
    let out = sys_dir.join("src/bindings.rs");
    // Kept with the build output, so a fetched checkout is reused across runs.
    let cache_dir = manifest_dir.join("../../target/ghostty-source");
    std::fs::create_dir_all(&cache_dir)
        .map_err(|error| format!("failed to create {}: {error}", cache_dir.display()))?;

    check_bindgen_version()?;
    let source = ghostty_source(&cache_dir)?;
    let include = source.join("include");

    let status = Command::new("bindgen")
        .arg(include.join("ghostty/vt.h"))
        .arg("--output")
        .arg(&out)
        .args(["--allowlist-function", "ghostty_.*"])
        .args(["--allowlist-type", "Ghostty.*"])
        .args(["--allowlist-var", "GHOSTTY_.*"])
        // Only sizes Ghostty's enums. Defined as `INT_MAX`, which only some
        // libclang builds evaluate.
        .args(["--blocklist-item", "GHOSTTY_ENUM_MAX_VALUE"])
        .args(["--default-enum-style", "moduleconsts"])
        .arg("--with-derive-default")
        .arg("--generate-cstr")
        // Formatted below instead, so a missing rustfmt fails the run rather
        // than leaving the bindings unformatted.
        .args(["--formatter", "none"])
        .arg("--raw-line")
        .arg(format!("// Generated from Ghostty {GHOSTTY_COMMIT}."))
        .arg("--")
        // Ghostty's headers give their enums a fixed `int` type in C23, and
        // otherwise only where the compiler offers it as an extension, which
        // depends on the libclang bindgen loads.
        .arg("-std=c23")
        .arg(format!("-I{}", include.display()))
        .status()
        .map_err(|error| format!("failed to run bindgen: {error}"))?;
    if !status.success() {
        return Err("bindgen failed".to_owned());
    }

    let status = Command::new("rustfmt")
        .args(["--edition", "2024"])
        .arg(&out)
        .status()
        .map_err(|error| format!("failed to run rustfmt: {error}"))?;
    if !status.success() {
        return Err(format!("rustfmt failed for {}", out.display()));
    }

    println!("wrote {}", out.display());
    Ok(())
}

fn check_bindgen_version() -> Result<(), String> {
    let output = Command::new("bindgen")
        .arg("--version")
        .output()
        .map_err(|error| {
            format!(
                "could not run bindgen ({error}); install bindgen-cli {BINDGEN_VERSION} and put \
                 it on PATH"
            )
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let reported = stdout.trim();
    if !output.status.success() || reported.strip_prefix("bindgen ") != Some(BINDGEN_VERSION) {
        return Err(format!(
            "bindgen {BINDGEN_VERSION} is required, but `bindgen --version` reported {reported:?}"
        ));
    }
    Ok(())
}
