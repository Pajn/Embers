use std::fs;

use embers_client::{
    BUILTIN_CONFIG_SOURCE, ConfigDiscoveryOptions, ConfigManager, ConfigOrigin, KeyToken,
};
use tempfile::tempdir;

#[test]
fn config_manager_loads_standard_config_file() {
    let tempdir = tempdir().unwrap();
    let config_path = tempdir.path().join("config.rhai");
    fs::write(&config_path, "set_leader(\"<C-a>\")").unwrap();
    let options = ConfigDiscoveryOptions::default().with_project_config_dir(tempdir.path());

    let manager = ConfigManager::load(options).unwrap();

    assert_eq!(manager.active_source().origin, ConfigOrigin::Standard);
    assert_eq!(
        manager.active_source().path,
        Some(config_path.canonicalize().unwrap())
    );
    assert_eq!(manager.active_source().source, "set_leader(\"<C-a>\")");
}

#[test]
fn explicit_override_wins_when_starting_manager() {
    let tempdir = tempdir().unwrap();
    let explicit_path = tempdir.path().join("explicit.rhai");
    let standard_path = tempdir.path().join("config.rhai");
    fs::write(&explicit_path, "set_leader(\"<C-b>\")").unwrap();
    fs::write(&standard_path, "set_leader(\"<C-c>\")").unwrap();
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
    assert_eq!(manager.active_source().source, "set_leader(\"<C-b>\")");
}

#[test]
fn manager_uses_builtin_config_when_no_files_exist() {
    let manager = ConfigManager::load(ConfigDiscoveryOptions {
        explicit_path: None,
        env_path: None,
        standard_config_path: None,
    })
    .unwrap();

    assert_eq!(manager.active_source().origin, ConfigOrigin::BuiltIn);
    assert_eq!(manager.active_source().source, BUILTIN_CONFIG_SOURCE);
    assert_eq!(manager.active_source().display_path(), "<built-in>");
    assert!(
        manager.active_script().loaded_config().bindings["normal"]
            .iter()
            .any(|binding| binding.notation == "<PageUp>")
    );
    assert!(manager.active_script().loaded_config().mouse.click_focus);
    assert!(manager.active_script().loaded_config().mouse.wheel_scroll);
}

#[test]
fn reload_keeps_previous_config_when_new_source_fails() {
    let tempdir = tempdir().unwrap();
    let config_path = tempdir.path().join("config.rhai");
    fs::write(&config_path, r#"set_leader("<C-a>")"#).unwrap();
    let options = ConfigDiscoveryOptions::default().with_project_config_dir(tempdir.path());

    let mut manager = ConfigManager::load(options).unwrap();
    let previous_source = manager.active_source().clone();

    fs::write(&config_path, "set_leader(").unwrap();
    let error = manager.reload().expect_err("reload must fail");

    assert!(error.to_string().contains("config.rhai"));
    assert_eq!(manager.active_source(), &previous_source);
    assert_eq!(
        manager.active_script().loaded_config().leader,
        vec![KeyToken::Ctrl('a')]
    );
}

#[test]
fn reload_swaps_in_new_compiled_config_on_success() {
    let tempdir = tempdir().unwrap();
    let config_path = tempdir.path().join("config.rhai");
    fs::write(&config_path, r#"set_leader("<C-a>")"#).unwrap();
    let options = ConfigDiscoveryOptions::default().with_project_config_dir(tempdir.path());

    let mut manager = ConfigManager::load(options).unwrap();
    fs::write(&config_path, r#"set_leader("<C-b>")"#).unwrap();

    manager.reload().unwrap();

    assert_eq!(manager.active_source().origin, ConfigOrigin::Standard);
    assert_eq!(
        manager.active_script().loaded_config().leader,
        vec![KeyToken::Ctrl('b')]
    );
}

#[test]
fn user_config_overlays_builtins_and_can_unbind_defaults() {
    let tempdir = tempdir().unwrap();
    let config_path = tempdir.path().join("config.rhai");
    fs::write(
        &config_path,
        r#"
            unbind("normal", "<PageUp>");
            mouse.set_wheel_scroll(false);
        "#,
    )
    .unwrap();

    let manager = ConfigManager::load(
        ConfigDiscoveryOptions::default().with_project_config_dir(tempdir.path()),
    )
    .unwrap();

    assert!(
        manager
            .active_source()
            .source
            .contains(r#"unbind("normal", "<PageUp>");"#)
    );
    assert!(
        manager
            .active_source()
            .source
            .contains("mouse.set_wheel_scroll(false);")
    );
    assert!(
        !manager.active_script().loaded_config().bindings["normal"]
            .iter()
            .any(|binding| binding.notation == "<PageUp>")
    );
    assert!(manager.active_script().loaded_config().mouse.click_focus);
    assert!(!manager.active_script().loaded_config().mouse.wheel_scroll);
}
