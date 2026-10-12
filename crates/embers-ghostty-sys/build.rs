//! Builds libghostty-vt with Zig and links it statically.
//!
//! Needs Zig (see `ghostty_source::ZIG_VERSION`) on `PATH`, and git and
//! network access unless `GHOSTTY_SOURCE_DIR` names a Ghostty checkout.
//! Zig fetches Ghostty's own package dependencies too, unless
//! `GHOSTTY_ZIG_SYSTEM_DIR` names a directory that already holds them (as
//! the Nix dev shell provides).

#[allow(dead_code)]
mod ghostty_source;

use std::env;
use std::path::PathBuf;
use std::process::Command;

use ghostty_source::{SOURCE_DIR_ENV, ZIG_VERSION, ghostty_source};

const ZIG_SYSTEM_DIR_ENV: &str = "GHOSTTY_ZIG_SYSTEM_DIR";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=ghostty_source.rs");
    println!("cargo:rerun-if-env-changed={SOURCE_DIR_ENV}");
    println!("cargo:rerun-if-env-changed={ZIG_SYSTEM_DIR_ENV}");

    if let Err(message) = build() {
        panic!("{message}");
    }
}

fn build() -> Result<(), String> {
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    let target = env::var("TARGET").expect("cargo sets TARGET");
    let host = env::var("HOST").expect("cargo sets HOST");
    if target != host {
        return Err(format!(
            "building libghostty-vt for {target} on {host} is not supported: build on the target"
        ));
    }

    check_zig_version()?;
    let source = ghostty_source(&out_dir)?;
    let prefix = out_dir.join("ghostty-vt");

    // Zig's debug builds are slow enough to matter in tests, so even a debug
    // Cargo build gets an optimized library, with Zig's safety checks kept.
    let optimize = match env::var("OPT_LEVEL").as_deref() {
        Ok("0") => "ReleaseSafe",
        _ => "ReleaseFast",
    };
    let mut zig = Command::new("zig");
    zig.arg("build")
        .arg("-Demit-lib-vt=true")
        .arg("-Demit-xcframework=false")
        .arg("-Dapp-runtime=none")
        .arg(format!("-Doptimize={optimize}"))
        // Built binaries are distributed, so they cannot assume the build
        // machine's CPU features.
        .arg("-Dcpu=baseline")
        .arg("--prefix")
        .arg(&prefix)
        .arg("--cache-dir")
        .arg(out_dir.join("zig-cache"))
        .current_dir(&source);
    if let Some(dir) = env::var_os(ZIG_SYSTEM_DIR_ENV) {
        zig.arg("--system")
            .arg(dir)
            .arg("--global-cache-dir")
            .arg(out_dir.join("zig-global-cache"));
    }
    let status = zig
        .status()
        .map_err(|error| format!("could not run zig: {error}"))?;
    if !status.success() {
        return Err(format!("`zig build` in {} {status}", source.display()));
    }

    let lib_dir = prefix.join("lib");
    if !lib_dir.join("libghostty-vt.a").exists() {
        return Err(format!(
            "zig build left no libghostty-vt.a in {}",
            lib_dir.display()
        ));
    }
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static=ghostty-vt");
    println!("cargo:include={}", prefix.join("include").display());
    Ok(())
}

fn check_zig_version() -> Result<(), String> {
    let output = Command::new("zig")
        .arg("version")
        .output()
        .map_err(|error| format!("could not run zig ({error}); Zig {ZIG_VERSION} is required"))?;
    let reported = String::from_utf8_lossy(&output.stdout);
    let reported = reported.trim();
    if !output.status.success() || reported != ZIG_VERSION {
        return Err(format!(
            "Zig {ZIG_VERSION} is required, but `zig version` reported {reported:?}"
        ));
    }
    Ok(())
}
