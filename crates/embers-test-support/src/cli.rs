pub fn cargo_bin(name: &str) -> assert_cmd::Command {
    assert_cmd::Command::cargo_bin(name)
        .unwrap_or_else(|error| panic!("failed to load binary {name}: {error}"))
}
