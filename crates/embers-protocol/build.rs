use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let schema = manifest_dir.join("schema/embers.fbs");
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));

    println!("cargo:rerun-if-changed={}", schema.display());

    let status = Command::new("flatc")
        .arg("--rust")
        .arg("-o")
        .arg(&out_dir)
        .arg(&schema)
        .status()
        .expect("flatc must be installed to build embers-protocol");

    assert!(status.success(), "flatc failed for {}", schema.display());
}
