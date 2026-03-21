use std::path::PathBuf;

pub fn cargo_bin(name: &str) -> assert_cmd::Command {
    assert_cmd::Command::cargo_bin(name)
        .unwrap_or_else(|error| panic!("failed to load binary {name}: {error}"))
}

pub fn cargo_bin_path(name: &str) -> PathBuf {
    assert_cmd::cargo::cargo_bin(name)
}
