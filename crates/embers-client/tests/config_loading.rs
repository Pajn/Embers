use std::fs;

use embers_client::{BUILTIN_CONFIG_SOURCE, ConfigDiscoveryOptions, ConfigManager, ConfigOrigin};
use tempfile::tempdir;

#[test]
fn config_manager_loads_standard_config_file() {
    let tempdir = tempdir().unwrap();
    let config_path = tempdir.path().join("config.rhai");
    fs::write(&config_path, "set_leader(\"C-a\")").unwrap();
    let options = ConfigDiscoveryOptions::default().with_project_config_dir(tempdir.path());

    let manager = ConfigManager::load(options).unwrap();

    assert_eq!(manager.active_source().origin, ConfigOrigin::Standard);
    assert_eq!(
        manager.active_source().path,
        Some(config_path.canonicalize().unwrap())
    );
    assert_eq!(manager.active_source().source, "set_leader(\"C-a\")");
}

#[test]
fn explicit_override_wins_when_starting_manager() {
    let tempdir = tempdir().unwrap();
    let explicit_path = tempdir.path().join("explicit.rhai");
    let standard_path = tempdir.path().join("config.rhai");
    fs::write(&explicit_path, "set_leader(\"C-b\")").unwrap();
    fs::write(&standard_path, "set_leader(\"C-c\")").unwrap();
    let options = ConfigDiscoveryOptions {
        explicit_path: Some(explicit_path.clone()),
        env_path: Some(standard_path.clone()),
        standard_config_path: Some(standard_path),
    };

    let manager = ConfigManager::load(options).unwrap();

    assert_eq!(manager.active_source().origin, ConfigOrigin::Explicit);
    assert_eq!(
        manager.active_source().path,
        Some(explicit_path.canonicalize().unwrap())
    );
    assert_eq!(manager.active_source().source, "set_leader(\"C-b\")");
}

#[test]
fn manager_uses_builtin_config_when_no_files_exist() {
    let manager = ConfigManager::load(ConfigDiscoveryOptions::default()).unwrap();

    assert_eq!(manager.active_source().origin, ConfigOrigin::BuiltIn);
    assert_eq!(manager.active_source().source, BUILTIN_CONFIG_SOURCE);
    assert_eq!(manager.active_source().display_path(), "<built-in>");
}
