//! Regenerates `embers-protocol`'s checked-in FlatBuffers code from its schema.
//!
//! This lives in its own crate rather than as a bin of `embers-protocol`, so it still builds
//! when a schema change has left the protocol crate failing to compile.

use std::path::PathBuf;
use std::process::{Command, ExitCode};

/// The `flatc` release the checked-in code is generated with. Keep it in step with the
/// `flatbuffers` runtime pin in the workspace `Cargo.toml` and `FLATBUFFERS_VERSION` in CI.
const FLATC_VERSION: &str = "25.12.19";

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
    let protocol_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../embers-protocol");
    let schema = protocol_dir.join("schema/embers.fbs");
    let out_dir = protocol_dir.join("src/generated");

    check_flatc_version()?;

    let status = Command::new("flatc")
        .arg("--rust")
        .arg("-o")
        .arg(&out_dir)
        .arg(&schema)
        .status()
        .map_err(|error| format!("failed to run flatc: {error}"))?;
    if !status.success() {
        return Err(format!("flatc failed for {}", schema.display()));
    }

    println!("wrote {}", out_dir.join("embers_generated.rs").display());
    Ok(())
}

fn check_flatc_version() -> Result<(), String> {
    let output = Command::new("flatc")
        .arg("--version")
        .output()
        .map_err(|error| {
            format!(
                "could not run flatc ({error}); install flatc from FlatBuffers {FLATC_VERSION} \
                 and put it on PATH"
            )
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let reported = stdout.trim();
    if !output.status.success() || reported.strip_prefix("flatc version ") != Some(FLATC_VERSION) {
        return Err(format!(
            "flatc {FLATC_VERSION} is required, but `flatc --version` reported {reported:?}"
        ));
    }
    Ok(())
}
