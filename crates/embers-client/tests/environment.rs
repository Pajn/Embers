//! `action.set_environment` / `action.set_global_environment`.

use std::fs;

use embers_client::{
    ConfigDiscoveryOptions, ConfigManager, ConfiguredClient, KeyEvent, MuxClient, ScriptedTransport,
};
use embers_core::{RequestId, Size};
use embers_protocol::{ClientMessage, OkResponse, ServerResponse, SessionRequest};
use tempfile::tempdir;

use crate::support::{SESSION_ID, demo_state};

const VIEWPORT: Size = Size {
    width: 80,
    height: 20,
};

#[tokio::test]
async fn environment_actions_set_and_unset_session_and_global_variables() {
    let transport = ScriptedTransport::default();
    for (id, session_id, name, value) in [
        (1, Some(SESSION_ID), "AWS_PROFILE", Some("staging")),
        (2, None, "EDITOR", Some("nvim")),
        (3, Some(SESSION_ID), "AWS_PROFILE", None),
    ] {
        transport.push_exchange(
            ClientMessage::Session(SessionRequest::SetEnvironment {
                request_id: RequestId(id),
                session_id,
                name: name.to_owned(),
                value: value.map(str::to_owned),
            }),
            ServerResponse::Ok(OkResponse {
                request_id: RequestId(id),
            }),
        );
    }
    let tempdir = tempdir().unwrap();
    fs::write(
        tempdir.path().join("config.rhai"),
        r#"
        bind("normal", "<M-1>", action.set_environment("AWS_PROFILE", "staging"));
        bind("normal", "<M-2>", action.set_global_environment("EDITOR", "nvim"));
        bind("normal", "<M-3>", action.set_environment("AWS_PROFILE", ()));
        "#,
    )
    .unwrap();
    let config = ConfigManager::load(
        ConfigDiscoveryOptions::default().with_project_config_dir(tempdir.path()),
    )
    .unwrap();
    let mut configured = ConfiguredClient::new(MuxClient::new(transport.clone()), config);
    *configured.client_mut().state_mut() = demo_state();

    for key in ['1', '2', '3'] {
        configured
            .handle_key(SESSION_ID, VIEWPORT, KeyEvent::Alt(key))
            .await
            .unwrap();
    }

    assert!(
        configured.notifications().is_empty(),
        "{:?}",
        configured.notifications()
    );
    transport.assert_exhausted().unwrap();
}
