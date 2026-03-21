use std::collections::BTreeMap;

use tracing::warn;

use embers_core::{BufferId, MuxError, Result, SessionId, Size};
use embers_protocol::{
    BufferRequest, BufferResponse, ClientMessage, FloatingRequest, InputRequest, NodeRequest,
    ServerEvent, ServerResponse, SessionRequest,
};

use crate::RenderGrid;
use crate::client::MuxClient;
use crate::config::ConfigManager;
use crate::controller::KeyEvent;
use crate::input::{
    FallbackPolicy, InputResolution, InputState, KeyToken, NORMAL_MODE, resolve_key,
};
use crate::presentation::PresentationModel;
use crate::renderer::Renderer;
use crate::scripting::{Action, BarSpec, BufferTarget, Context, TabBarContext, TreeSpec};
use crate::transport::Transport;

pub struct ConfiguredClient<T> {
    client: MuxClient<T>,
    config: ConfigManager,
    input_state: InputState,
    renderer: Renderer,
    notifications: Vec<String>,
    active_session_id: Option<SessionId>,
    viewport: Option<Size>,
}

impl<T> ConfiguredClient<T>
where
    T: Transport,
{
    pub fn new(client: MuxClient<T>, config: ConfigManager) -> Self {
        Self {
            client,
            config,
            input_state: InputState::default(),
            renderer: Renderer,
            notifications: Vec::new(),
            active_session_id: None,
            viewport: None,
        }
    }

    pub fn client(&self) -> &MuxClient<T> {
        &self.client
    }

    pub fn client_mut(&mut self) -> &mut MuxClient<T> {
        &mut self.client
    }

    pub fn config(&self) -> &ConfigManager {
        &self.config
    }

    pub fn notifications(&self) -> &[String] {
        &self.notifications
    }

    pub async fn handle_key(
        &mut self,
        session_id: SessionId,
        viewport: Size,
        key: KeyEvent,
    ) -> Result<()> {
        self.set_active_view(session_id, viewport);
        let presentation = self.prepare_presentation(session_id, viewport).await?;

        match key {
            KeyEvent::Bytes(bytes) => {
                self.send_bytes(BufferTarget::Current, session_id, &presentation, bytes)
                    .await?;
                Ok(())
            }
            other => {
                let token = key_event_to_token(other)?;
                match resolve_key(
                    &self.config.active_script().loaded_config().bindings,
                    &self.config.active_script().loaded_config().modes,
                    &mut self.input_state,
                    token,
                ) {
                    InputResolution::ExactMatch(binding) => {
                        let context = Context::from_state(self.client.state(), Some(&presentation));
                        match self
                            .config
                            .active_script()
                            .run_named_action(&binding.target, context)
                        {
                            Ok(actions) => {
                                self.execute_actions(session_id, viewport, actions).await
                            }
                            Err(error) => {
                                self.record_notification(error.to_string());
                                Ok(())
                            }
                        }
                    }
                    InputResolution::PrefixMatch => Ok(()),
                    InputResolution::Unmatched {
                        sequence,
                        fallback_policy,
                        ..
                    } => match fallback_policy {
                        FallbackPolicy::Passthrough => {
                            self.send_bytes(
                                BufferTarget::Current,
                                session_id,
                                &presentation,
                                sequence_to_bytes(&sequence)?,
                            )
                            .await
                        }
                        FallbackPolicy::Ignore => Ok(()),
                    },
                }
            }
        }
    }

    pub async fn process_next_event(&mut self) -> Result<ServerEvent> {
        let event = self.client.process_next_event().await?;
        if let ServerEvent::RenderInvalidated(event) = &event {
            self.client.refresh_buffer_snapshot(event.buffer_id).await?;
        }

        let context = self.current_context();
        match self
            .config
            .active_script()
            .dispatch_event(event_name(&event), context)
        {
            Ok(actions) if !actions.is_empty() => {
                let session_id = self
                    .active_session_id
                    .or_else(|| event.session_id())
                    .or_else(|| self.client.state().sessions.keys().next().copied());
                if let (Some(session_id), Some(viewport)) = (session_id, self.viewport) {
                    self.execute_actions(session_id, viewport, actions).await?;
                }
            }
            Ok(_) => {}
            Err(error) => self.record_notification(error.to_string()),
        }
        Ok(event)
    }

    pub async fn render_session(
        &mut self,
        session_id: SessionId,
        viewport: Size,
    ) -> Result<RenderGrid> {
        self.set_active_view(session_id, viewport);
        let presentation = self.prepare_presentation(session_id, viewport).await?;
        let context = Context::from_state(self.client.state(), Some(&presentation));
        let mut custom_bars = BTreeMap::<embers_core::NodeId, BarSpec>::new();
        for tabs in &presentation.tab_bars {
            let bar_context = TabBarContext::from_frame(tabs);
            let result = if tabs.is_root {
                self.config
                    .active_script()
                    .format_root_tabbar(context.clone(), bar_context)
            } else {
                self.config
                    .active_script()
                    .format_nested_tabbar(context.clone(), bar_context)
            };

            match result {
                Ok(Some(bar)) => {
                    custom_bars.insert(tabs.node_id, bar);
                }
                Ok(None) => {}
                Err(error) => self.record_notification(error.to_string()),
            }
        }

        Ok(self
            .renderer
            .render_with_tab_bars(self.client.state(), &presentation, &custom_bars))
    }

    pub fn reload_config(&mut self) -> Result<()> {
        let current_mode = self.input_state.current_mode().to_owned();
        self.config
            .reload()
            .map_err(|error| MuxError::invalid_input(error.to_string()))?;
        if self
            .config
            .active_script()
            .loaded_config()
            .modes
            .contains_key(&current_mode)
        {
            self.input_state.clear_pending();
        } else {
            self.input_state.set_mode(NORMAL_MODE);
        }
        Ok(())
    }

    async fn execute_actions(
        &mut self,
        session_id: SessionId,
        viewport: Size,
        actions: Vec<Action>,
    ) -> Result<()> {
        for action in actions {
            let presentation = self.prepare_presentation(session_id, viewport).await?;
            if let Err(error) = self
                .execute_action(session_id, viewport, &presentation, action)
                .await
            {
                self.record_notification(error.to_string());
            }
        }
        Ok(())
    }

    async fn execute_action(
        &mut self,
        session_id: SessionId,
        _viewport: Size,
        presentation: &PresentationModel,
        action: Action,
    ) -> Result<()> {
        match action {
            Action::EnterMode { mode } => {
                if self
                    .config
                    .active_script()
                    .loaded_config()
                    .modes
                    .contains_key(&mode)
                {
                    self.input_state.set_mode(mode);
                    Ok(())
                } else {
                    Err(MuxError::invalid_input(format!("unknown mode '{mode}'")))
                }
            }
            Action::Focus { direction } => {
                let Some(node_id) = presentation.focus_target(direction) else {
                    return Ok(());
                };
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::Focus {
                        request_id: self.client.next_request_id(),
                        session_id,
                        node_id,
                    }))
                    .await?;
                self.client.resync_session(session_id).await
            }
            Action::SelectTab { index } => {
                let tabs = presentation
                    .focused_tabs()
                    .unwrap_or(&presentation.root_tabs);
                if index >= tabs.tabs.len() {
                    return Err(MuxError::invalid_input(format!(
                        "tab index {index} is out of range for {} tabs",
                        tabs.tabs.len()
                    )));
                }
                if tabs.is_root {
                    self.client
                        .request_message(ClientMessage::Session(SessionRequest::SelectRootTab {
                            request_id: self.client.next_request_id(),
                            session_id,
                            index,
                        }))
                        .await?;
                } else {
                    self.client
                        .request_message(ClientMessage::Node(NodeRequest::SelectTab {
                            request_id: self.client.next_request_id(),
                            tabs_node_id: tabs.node_id,
                            index,
                        }))
                        .await?;
                }
                self.client.resync_session(session_id).await
            }
            Action::Split { direction, tree } => {
                let focused_leaf = presentation
                    .focused_leaf()
                    .ok_or_else(|| MuxError::invalid_input("no focused leaf to split"))?;
                let buffer_id = self
                    .resolve_tree_buffer(session_id, presentation, tree)
                    .await?;
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::Split {
                        request_id: self.client.next_request_id(),
                        leaf_node_id: focused_leaf.node_id,
                        direction,
                        new_buffer_id: buffer_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await
            }
            Action::OpenFloating { tree, options } => {
                let buffer_id = self
                    .resolve_tree_buffer(session_id, presentation, tree)
                    .await?;
                self.client
                    .request_message(ClientMessage::Floating(FloatingRequest::Create {
                        request_id: self.client.next_request_id(),
                        session_id,
                        root_node_id: None,
                        buffer_id: Some(buffer_id),
                        geometry: options.geometry,
                        title: options.title,
                    }))
                    .await?;
                self.client.resync_all_sessions().await
            }
            Action::DetachBuffer { target } => {
                let buffer_id = self.resolve_buffer_target(target, presentation)?;
                self.client
                    .request_message(ClientMessage::Buffer(BufferRequest::Detach {
                        request_id: self.client.next_request_id(),
                        buffer_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await
            }
            Action::KillBuffer { target, force } => {
                let buffer_id = self.resolve_buffer_target(target, presentation)?;
                self.client
                    .request_message(ClientMessage::Buffer(BufferRequest::Kill {
                        request_id: self.client.next_request_id(),
                        buffer_id,
                        force,
                    }))
                    .await?;
                self.client.resync_all_sessions().await
            }
            Action::SendBytes { target, bytes } => {
                self.send_bytes(target, session_id, presentation, bytes)
                    .await
            }
            Action::Notify { message } => {
                self.record_notification(message);
                Ok(())
            }
            Action::ReloadConfig => self.reload_config(),
            other => Err(MuxError::invalid_input(format!(
                "action '{other:?}' is not supported by the live executor yet"
            ))),
        }
    }

    async fn resolve_tree_buffer(
        &self,
        _session_id: SessionId,
        presentation: &PresentationModel,
        tree: TreeSpec,
    ) -> Result<BufferId> {
        match tree {
            TreeSpec::BufferCurrent => presentation
                .focused_buffer_id()
                .ok_or_else(|| MuxError::invalid_input("no current buffer is focused")),
            TreeSpec::BufferAttach { buffer_id } => Ok(buffer_id),
            TreeSpec::BufferSpawn(spec) => self.create_buffer(spec).await,
            TreeSpec::BufferEmpty => {
                self.create_buffer(crate::scripting::BufferSpawnSpec {
                    title: Some("empty".to_owned()),
                    command: Vec::new(),
                    cwd: None,
                })
                .await
            }
            other => Err(MuxError::invalid_input(format!(
                "tree '{other:?}' is not supported by the live executor yet"
            ))),
        }
    }

    async fn create_buffer(&self, spec: crate::scripting::BufferSpawnSpec) -> Result<BufferId> {
        let response = self
            .client
            .request_message(ClientMessage::Buffer(BufferRequest::Create {
                request_id: self.client.next_request_id(),
                title: spec.title,
                command: spec.command,
                cwd: spec.cwd,
            }))
            .await?;

        match response {
            ServerResponse::Buffer(BufferResponse { buffer, .. }) => Ok(buffer.id),
            other => Err(MuxError::protocol(format!(
                "expected buffer response, got {other:?}"
            ))),
        }
    }

    async fn send_bytes(
        &mut self,
        target: BufferTarget,
        session_id: SessionId,
        presentation: &PresentationModel,
        bytes: Vec<u8>,
    ) -> Result<()> {
        let buffer_id = self.resolve_buffer_target(target, presentation)?;
        self.client
            .request_message(ClientMessage::Input(InputRequest::Send {
                request_id: self.client.next_request_id(),
                buffer_id,
                bytes,
            }))
            .await?;
        self.client.refresh_buffer_snapshot(buffer_id).await?;
        self.client.resync_session(session_id).await
    }

    fn resolve_buffer_target(
        &self,
        target: BufferTarget,
        presentation: &PresentationModel,
    ) -> Result<BufferId> {
        match target {
            BufferTarget::Current => presentation
                .focused_buffer_id()
                .ok_or_else(|| MuxError::invalid_input("no current buffer is focused")),
            BufferTarget::Buffer(buffer_id) => Ok(buffer_id),
        }
    }

    async fn prepare_presentation(
        &mut self,
        session_id: SessionId,
        viewport: Size,
    ) -> Result<PresentationModel> {
        let mut presentation =
            PresentationModel::project(self.client.state(), session_id, viewport)?;
        let invalidated = presentation
            .leaves
            .iter()
            .filter(|leaf| {
                self.client
                    .state()
                    .invalidated_buffers
                    .contains(&leaf.buffer_id)
            })
            .map(|leaf| leaf.buffer_id)
            .collect::<Vec<_>>();
        for buffer_id in invalidated {
            self.client.refresh_buffer_snapshot(buffer_id).await?;
        }
        if !self.client.state().invalidated_buffers.is_empty() {
            presentation = PresentationModel::project(self.client.state(), session_id, viewport)?;
        }
        Ok(presentation)
    }

    fn current_context(&self) -> Context {
        if let (Some(session_id), Some(viewport)) = (self.active_session_id, self.viewport)
            && let Ok(presentation) =
                PresentationModel::project(self.client.state(), session_id, viewport)
        {
            return Context::from_state(self.client.state(), Some(&presentation));
        }
        Context::from_state(self.client.state(), None)
    }

    fn set_active_view(&mut self, session_id: SessionId, viewport: Size) {
        self.active_session_id = Some(session_id);
        self.viewport = Some(viewport);
    }

    fn record_notification(&mut self, message: impl Into<String>) {
        let message = message.into();
        warn!("{message}");
        self.notifications.push(message);
        if self.notifications.len() > 64 {
            let overflow = self.notifications.len() - 64;
            self.notifications.drain(0..overflow);
        }
    }
}

fn key_event_to_token(key: KeyEvent) -> Result<KeyToken> {
    match key {
        KeyEvent::Char(ch) => Ok(KeyToken::Char(ch)),
        KeyEvent::Enter => Ok(KeyToken::Enter),
        KeyEvent::Tab => Ok(KeyToken::Tab),
        KeyEvent::Backspace => Ok(KeyToken::Backspace),
        KeyEvent::Escape => Ok(KeyToken::Escape),
        KeyEvent::Ctrl(ch) => Ok(KeyToken::Ctrl(ch.to_ascii_lowercase())),
        KeyEvent::Alt(ch) => Ok(KeyToken::Alt(ch.to_ascii_lowercase())),
        KeyEvent::Bytes(_) => Err(MuxError::invalid_input("raw bytes are handled separately")),
    }
}

fn sequence_to_bytes(sequence: &[KeyToken]) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    for token in sequence {
        match token {
            KeyToken::Char(ch) => {
                let mut encoded = [0; 4];
                bytes.extend_from_slice(ch.encode_utf8(&mut encoded).as_bytes());
            }
            KeyToken::Space => bytes.push(b' '),
            KeyToken::Tab => bytes.push(b'\t'),
            KeyToken::Enter => bytes.push(b'\r'),
            KeyToken::Backspace => bytes.push(0x7f),
            KeyToken::Escape => bytes.push(0x1b),
            KeyToken::Ctrl(ch) => bytes.push(ctrl_byte(*ch)?),
            KeyToken::Alt(ch) => {
                bytes.push(0x1b);
                bytes.extend(sequence_to_bytes(&[KeyToken::Char(*ch)])?);
            }
            KeyToken::Leader => {
                return Err(MuxError::invalid_input(
                    "leader placeholders cannot be sent directly",
                ));
            }
        }
    }
    Ok(bytes)
}

fn ctrl_byte(ch: char) -> Result<u8> {
    if !ch.is_ascii() {
        return Err(MuxError::invalid_input("control keys must be ASCII"));
    }
    Ok((ch.to_ascii_lowercase() as u8) & 0x1f)
}

fn event_name(event: &ServerEvent) -> &'static str {
    match event {
        ServerEvent::SessionCreated(_) => "session-created",
        ServerEvent::SessionClosed(_) => "session-closed",
        ServerEvent::BufferCreated(_) => "buffer-created",
        ServerEvent::BufferDetached(_) => "buffer-detached",
        ServerEvent::NodeChanged(_) => "node-changed",
        ServerEvent::FloatingChanged(_) => "floating-changed",
        ServerEvent::FocusChanged(_) => "focus-changed",
        ServerEvent::RenderInvalidated(_) => "render-invalidated",
    }
}
