use std::collections::{BTreeMap, VecDeque};

use tracing::warn;

use embers_core::{ActivityState, BufferId, FloatGeometry, MuxError, Result, SessionId, Size};
use embers_protocol::{
    BufferRequest, BufferResponse, ClientMessage, FloatingRequest, InputRequest, NodeRequest,
    ServerEvent, ServerResponse,
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
use crate::scripting::{
    Action, BarSpec, Context, EventInfo, FloatingAnchor, FloatingGeometrySpec, FloatingSize,
    NotifyLevel, TabBarContext, TreeSpec,
};
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
                let buffer_id = self.resolve_buffer_id(None, &presentation)?;
                self.send_bytes_to_buffer(buffer_id, session_id, bytes)
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
                        self.execute_actions(
                            Some(session_id),
                            Some(viewport),
                            binding.target.clone(),
                        )
                        .await
                    }
                    InputResolution::PrefixMatch => Ok(()),
                    InputResolution::Unmatched {
                        sequence,
                        fallback_policy,
                        ..
                    } => match fallback_policy {
                        FallbackPolicy::Passthrough => {
                            let buffer_id = self.resolve_buffer_id(None, &presentation)?;
                            self.send_bytes_to_buffer(
                                buffer_id,
                                session_id,
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

    pub async fn handle_paste(
        &mut self,
        session_id: SessionId,
        viewport: Size,
        bytes: Vec<u8>,
    ) -> Result<()> {
        self.set_active_view(session_id, viewport);
        if self.current_fallback_policy() != FallbackPolicy::Passthrough {
            return Ok(());
        }

        let presentation = self.prepare_presentation(session_id, viewport).await?;
        let buffer_id = self.resolve_buffer_id(None, &presentation)?;
        let bytes = if self
            .client
            .state()
            .snapshots
            .get(&buffer_id)
            .is_some_and(|snapshot| snapshot.bracketed_paste)
        {
            let mut wrapped = b"\x1b[200~".to_vec();
            wrapped.extend(bytes);
            wrapped.extend_from_slice(b"\x1b[201~");
            wrapped
        } else {
            bytes
        };
        self.send_bytes_to_buffer(buffer_id, session_id, bytes)
            .await
    }

    pub async fn handle_focus_event(
        &mut self,
        session_id: SessionId,
        viewport: Size,
        focused: bool,
    ) -> Result<()> {
        self.set_active_view(session_id, viewport);
        let presentation = self.prepare_presentation(session_id, viewport).await?;
        let buffer_id = self.resolve_buffer_id(None, &presentation)?;
        if !self
            .client
            .state()
            .snapshots
            .get(&buffer_id)
            .is_some_and(|snapshot| snapshot.focus_reporting)
        {
            return Ok(());
        }

        let bytes = if focused {
            b"\x1b[I".to_vec()
        } else {
            b"\x1b[O".to_vec()
        };
        self.send_bytes_to_buffer(buffer_id, session_id, bytes)
            .await
    }

    pub async fn process_next_event(&mut self) -> Result<ServerEvent> {
        let event = self.client.process_next_event().await?;
        if let ServerEvent::RenderInvalidated(event) = &event {
            self.client.refresh_buffer_snapshot(event.buffer_id).await?;
        }

        let session_id = self
            .active_session_id
            .or_else(|| event.session_id())
            .or_else(|| self.client.state().sessions.keys().next().copied());
        let mut event_names = vec![event_name(&event).to_owned()];
        if let ServerEvent::RenderInvalidated(render) = &event
            && self
                .client
                .state()
                .buffers
                .get(&render.buffer_id)
                .is_some_and(|buffer| buffer.activity == ActivityState::Bell)
        {
            event_names.push("buffer_bell".to_owned());
        }

        for event_name in event_names {
            let context = self.context_for(
                session_id,
                self.viewport,
                Some(event_info(&event_name, &event)),
            );
            match self
                .config
                .active_script()
                .dispatch_event(&event_name, context)
            {
                Ok(actions) if !actions.is_empty() => {
                    self.execute_actions(session_id, self.viewport, actions)
                        .await?;
                }
                Ok(_) => {}
                Err(error) => self.record_notification(error.to_string()),
            }
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
        let mut custom_bars = BTreeMap::<embers_core::NodeId, BarSpec>::new();
        let mut recorded_formatter_error = false;
        for tabs in &presentation.tab_bars {
            let bar_context =
                TabBarContext::from_frame(tabs, self.input_state.current_mode(), viewport.width);
            let result = self.config.active_script().format_tab_bar(bar_context);

            match result {
                Ok(Some(bar)) => {
                    custom_bars.insert(tabs.node_id, bar);
                }
                Ok(None) => {}
                Err(error) if !recorded_formatter_error => {
                    recorded_formatter_error = true;
                    self.record_notification(error.to_string());
                }
                Err(_) => {}
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
        session_id: Option<SessionId>,
        viewport: Option<Size>,
        actions: Vec<Action>,
    ) -> Result<()> {
        let mut pending = VecDeque::from(actions);
        while let Some(action) = pending.pop_front() {
            let result = match action {
                Action::Noop => Ok(()),
                Action::Chain(actions) => {
                    prepend_actions(&mut pending, actions);
                    Ok(())
                }
                Action::RunNamedAction { name } => {
                    match self
                        .config
                        .active_script()
                        .run_named_action(&name, self.context_for(session_id, viewport, None))
                    {
                        Ok(actions) => {
                            prepend_actions(&mut pending, actions);
                            Ok(())
                        }
                        Err(error) => Err(MuxError::invalid_input(error.to_string())),
                    }
                }
                Action::EnterMode { mode } => {
                    let actions = self.transition_mode(mode, session_id, viewport).await?;
                    prepend_actions(&mut pending, actions);
                    Ok(())
                }
                Action::LeaveMode => {
                    let actions = self
                        .transition_mode(NORMAL_MODE.to_owned(), session_id, viewport)
                        .await?;
                    prepend_actions(&mut pending, actions);
                    Ok(())
                }
                Action::ToggleMode { mode } => {
                    let next_mode = if self.input_state.current_mode() == mode {
                        NORMAL_MODE.to_owned()
                    } else {
                        mode
                    };
                    let actions = self
                        .transition_mode(next_mode, session_id, viewport)
                        .await?;
                    prepend_actions(&mut pending, actions);
                    Ok(())
                }
                Action::ClearPendingKeys => {
                    self.input_state.clear_pending();
                    Ok(())
                }
                Action::Notify { level, message } => {
                    self.record_notification(format_notification(level, &message));
                    Ok(())
                }
                action => {
                    let Some((session_id, viewport)) = session_id.zip(viewport) else {
                        continue;
                    };
                    let presentation = self.prepare_presentation(session_id, viewport).await?;
                    self.execute_action(session_id, viewport, &presentation, action)
                        .await
                }
            };
            if let Err(error) = result {
                self.record_notification(error.to_string());
            }
        }
        Ok(())
    }

    async fn transition_mode(
        &mut self,
        mode: String,
        session_id: Option<SessionId>,
        viewport: Option<Size>,
    ) -> Result<Vec<Action>> {
        if !self
            .config
            .active_script()
            .loaded_config()
            .modes
            .contains_key(&mode)
        {
            return Err(MuxError::invalid_input(format!("unknown mode '{mode}'")));
        }

        let previous_mode = self.input_state.current_mode().to_owned();
        if previous_mode == mode {
            return Ok(Vec::new());
        }

        let mut actions = self
            .config
            .active_script()
            .run_leave_hook(&previous_mode, self.context_for(session_id, viewport, None))
            .map_err(|error| MuxError::invalid_input(error.to_string()))?;
        self.input_state.set_mode(mode.clone());
        actions.extend(
            self.config
                .active_script()
                .run_enter_hook(&mode, self.context_for(session_id, viewport, None))
                .map_err(|error| MuxError::invalid_input(error.to_string()))?,
        );
        Ok(actions)
    }

    async fn execute_action(
        &mut self,
        session_id: SessionId,
        viewport: Size,
        presentation: &PresentationModel,
        action: Action,
    ) -> Result<()> {
        match action {
            Action::FocusDirection { direction } => {
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
            Action::ResizeDirection { .. } => Err(MuxError::invalid_input(
                "resize actions are not implemented yet",
            )),
            Action::SelectTab {
                tabs_node_id,
                index,
            } => {
                let tabs = self.resolve_tabs_target(presentation, tabs_node_id)?;
                if index >= tabs.tabs.len() {
                    return Err(MuxError::invalid_input(format!(
                        "tab index {index} is out of range for {} tabs",
                        tabs.tabs.len()
                    )));
                }
                let index = u32::try_from(index).map_err(|_| {
                    MuxError::invalid_input(format!("tab index {index} exceeds protocol limits"))
                })?;
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::SelectTab {
                        request_id: self.client.next_request_id(),
                        tabs_node_id: tabs.node_id,
                        index,
                    }))
                    .await?;
                self.client.resync_session(session_id).await
            }
            Action::NextTab { tabs_node_id } => {
                let tabs = self.resolve_tabs_target(presentation, tabs_node_id)?;
                if tabs.tabs.is_empty() {
                    return Ok(());
                }
                let index = (tabs.active + 1) % tabs.tabs.len();
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::SelectTab {
                        request_id: self.client.next_request_id(),
                        tabs_node_id: tabs.node_id,
                        index: u32::try_from(index).map_err(|_| {
                            MuxError::invalid_input("tab index exceeds protocol limits")
                        })?,
                    }))
                    .await?;
                self.client.resync_session(session_id).await
            }
            Action::PrevTab { tabs_node_id } => {
                let tabs = self.resolve_tabs_target(presentation, tabs_node_id)?;
                if tabs.tabs.is_empty() {
                    return Ok(());
                }
                let index = if tabs.active == 0 {
                    tabs.tabs.len() - 1
                } else {
                    tabs.active - 1
                };
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::SelectTab {
                        request_id: self.client.next_request_id(),
                        tabs_node_id: tabs.node_id,
                        index: u32::try_from(index).map_err(|_| {
                            MuxError::invalid_input("tab index exceeds protocol limits")
                        })?,
                    }))
                    .await?;
                self.client.resync_session(session_id).await
            }
            Action::SplitCurrent {
                direction,
                new_child,
            } => {
                let focused_leaf = presentation
                    .focused_leaf()
                    .ok_or_else(|| MuxError::invalid_input("no focused leaf to split"))?;
                let buffer_id = self
                    .resolve_tree_buffer(session_id, presentation, new_child)
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
            Action::OpenFloating { spec } => {
                let buffer_id = self
                    .resolve_tree_buffer(session_id, presentation, spec.tree)
                    .await?;
                self.client
                    .request_message(ClientMessage::Floating(FloatingRequest::Create {
                        request_id: self.client.next_request_id(),
                        session_id,
                        root_node_id: None,
                        buffer_id: Some(buffer_id),
                        geometry: resolve_floating_geometry(spec.geometry, viewport),
                        title: spec.title,
                        focus: spec.focus,
                        close_on_empty: spec.close_on_empty,
                    }))
                    .await?;
                self.client.resync_all_sessions().await
            }
            Action::DetachBuffer { buffer_id } => {
                let buffer_id = self.resolve_buffer_id(buffer_id, presentation)?;
                self.client
                    .request_message(ClientMessage::Buffer(BufferRequest::Detach {
                        request_id: self.client.next_request_id(),
                        buffer_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await
            }
            Action::KillBuffer { buffer_id } => {
                let buffer_id = self.resolve_buffer_id(buffer_id, presentation)?;
                self.client
                    .request_message(ClientMessage::Buffer(BufferRequest::Kill {
                        request_id: self.client.next_request_id(),
                        buffer_id,
                        force: false,
                    }))
                    .await?;
                self.client.resync_all_sessions().await
            }
            Action::SendKeys { buffer_id, keys } => {
                let buffer_id = self.resolve_buffer_id(buffer_id, presentation)?;
                self.send_bytes_to_buffer(buffer_id, session_id, sequence_to_bytes(&keys)?)
                    .await
            }
            Action::SendBytes { buffer_id, bytes } => {
                let buffer_id = self.resolve_buffer_id(buffer_id, presentation)?;
                self.send_bytes_to_buffer(buffer_id, session_id, bytes)
                    .await
            }
            Action::CloseFloating { floating_id } => {
                let floating_id = floating_id
                    .or_else(|| presentation.focused_floating_id())
                    .ok_or_else(|| MuxError::invalid_input("no floating window is focused"))?;
                self.client
                    .request_message(ClientMessage::Floating(FloatingRequest::Close {
                        request_id: self.client.next_request_id(),
                        floating_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await
            }
            Action::CloseView { node_id } => {
                let node_id = node_id
                    .or_else(|| presentation.focused_leaf().map(|leaf| leaf.node_id))
                    .ok_or_else(|| MuxError::invalid_input("no focused node to close"))?;
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::Close {
                        request_id: self.client.next_request_id(),
                        node_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await
            }
            Action::InsertTabAfter {
                tabs_node_id,
                title,
                child,
            } => {
                let tabs = self.resolve_tabs_target(presentation, tabs_node_id)?;
                let buffer_id = self
                    .resolve_tree_buffer(session_id, presentation, child)
                    .await?;
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::AddTab {
                        request_id: self.client.next_request_id(),
                        tabs_node_id: tabs.node_id,
                        title: title.unwrap_or_else(|| "tab".to_owned()),
                        buffer_id: Some(buffer_id),
                        child_node_id: None,
                        index: u32::try_from(tabs.active.saturating_add(1)).map_err(|_| {
                            MuxError::invalid_input("tab index exceeds protocol limits")
                        })?,
                    }))
                    .await?;
                self.client.resync_all_sessions().await
            }
            Action::InsertTabBefore {
                tabs_node_id,
                title,
                child,
            } => {
                let tabs = self.resolve_tabs_target(presentation, tabs_node_id)?;
                let buffer_id = self
                    .resolve_tree_buffer(session_id, presentation, child)
                    .await?;
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::AddTab {
                        request_id: self.client.next_request_id(),
                        tabs_node_id: tabs.node_id,
                        title: title.unwrap_or_else(|| "tab".to_owned()),
                        buffer_id: Some(buffer_id),
                        child_node_id: None,
                        index: u32::try_from(tabs.active).map_err(|_| {
                            MuxError::invalid_input("tab index exceeds protocol limits")
                        })?,
                    }))
                    .await?;
                self.client.resync_all_sessions().await
            }
            Action::ReplaceNode { node_id, tree } => {
                let node_id = node_id
                    .or_else(|| presentation.focused_leaf().map(|leaf| leaf.node_id))
                    .ok_or_else(|| MuxError::invalid_input("no focused node to replace"))?;
                let buffer_id = self
                    .resolve_tree_buffer(session_id, presentation, tree)
                    .await?;
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::MoveBufferToNode {
                        request_id: self.client.next_request_id(),
                        buffer_id,
                        target_leaf_node_id: node_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await
            }
            Action::MoveBufferToNode { buffer_id, node_id } => {
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::MoveBufferToNode {
                        request_id: self.client.next_request_id(),
                        buffer_id,
                        target_leaf_node_id: node_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await
            }
            Action::MoveBufferToFloating {
                buffer_id,
                geometry,
                title,
                focus,
            } => {
                self.client
                    .request_message(ClientMessage::Floating(FloatingRequest::Create {
                        request_id: self.client.next_request_id(),
                        session_id,
                        root_node_id: None,
                        buffer_id: Some(buffer_id),
                        geometry: resolve_floating_geometry(geometry, viewport),
                        title,
                        focus,
                        close_on_empty: true,
                    }))
                    .await?;
                self.client.resync_all_sessions().await
            }
            Action::FocusBuffer { buffer_id } | Action::RevealBuffer { buffer_id } => {
                self.focus_buffer(session_id, buffer_id).await
            }
            Action::ReplaceFloatingRoot { .. }
            | Action::WrapNodeInSplit { .. }
            | Action::WrapNodeInTabs { .. }
            | Action::CopySelection
            | Action::CancelSelection => Err(MuxError::invalid_input(format!(
                "action '{action:?}' is not supported by the live executor yet"
            ))),
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
                    title: Some("shell".to_owned()),
                    command: default_shell_command(),
                    cwd: None,
                    env: Default::default(),
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
                env: spec.env,
            }))
            .await?;

        match response {
            ServerResponse::Buffer(BufferResponse { buffer, .. }) => Ok(buffer.id),
            other => Err(MuxError::protocol(format!(
                "expected buffer response, got {other:?}"
            ))),
        }
    }

    async fn send_bytes_to_buffer(
        &mut self,
        buffer_id: BufferId,
        session_id: SessionId,
        bytes: Vec<u8>,
    ) -> Result<()> {
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

    fn resolve_buffer_id(
        &self,
        buffer_id: Option<BufferId>,
        presentation: &PresentationModel,
    ) -> Result<BufferId> {
        match buffer_id {
            Some(buffer_id) => Ok(buffer_id),
            None => presentation
                .focused_buffer_id()
                .ok_or_else(|| MuxError::invalid_input("no current buffer is focused")),
        }
    }

    fn resolve_tabs_target(
        &self,
        presentation: &PresentationModel,
        tabs_node_id: Option<embers_core::NodeId>,
    ) -> Result<crate::TabsFrame> {
        match tabs_node_id {
            Some(tabs_node_id) => presentation
                .tab_bars
                .iter()
                .find(|tabs| tabs.node_id == tabs_node_id)
                .cloned()
                .or_else(|| {
                    self.client
                        .state()
                        .nodes
                        .get(&tabs_node_id)
                        .and_then(|node| {
                            node.tabs.as_ref().map(|tabs| crate::TabsFrame {
                                node_id: tabs_node_id,
                                rect: embers_core::Rect::default(),
                                tabs: tabs
                                    .tabs
                                    .iter()
                                    .enumerate()
                                    .map(|(index, tab)| crate::TabItem {
                                        title: tab.title.clone(),
                                        child_id: tab.child_id,
                                        active: usize::try_from(tabs.active).ok() == Some(index),
                                        activity: ActivityState::Idle,
                                    })
                                    .collect(),
                                active: usize::try_from(tabs.active).unwrap_or(0),
                                is_root: self
                                    .client
                                    .state()
                                    .sessions
                                    .values()
                                    .any(|session| session.root_node_id == tabs_node_id),
                                floating_id: None,
                            })
                        })
                })
                .ok_or_else(|| MuxError::invalid_input(format!("node {tabs_node_id} is not tabs"))),
            None => presentation
                .focused_tabs()
                .cloned()
                .ok_or_else(|| MuxError::invalid_input("no focused tabs to select from")),
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

    fn context_for(
        &self,
        session_id: Option<SessionId>,
        viewport: Option<Size>,
        event: Option<EventInfo>,
    ) -> Context {
        let context = if let Some((session_id, viewport)) = session_id.zip(viewport)
            && let Ok(presentation) =
                PresentationModel::project(self.client.state(), session_id, viewport)
        {
            Context::from_state_with_mode(
                self.client.state(),
                Some(&presentation),
                self.input_state.current_mode(),
            )
        } else {
            Context::from_state_with_mode(
                self.client.state(),
                None,
                self.input_state.current_mode(),
            )
        };
        if let Some(event) = event {
            context.with_event(event)
        } else {
            context
        }
    }

    fn set_active_view(&mut self, session_id: SessionId, viewport: Size) {
        self.active_session_id = Some(session_id);
        self.viewport = Some(viewport);
    }

    fn current_fallback_policy(&self) -> FallbackPolicy {
        self.config
            .active_script()
            .loaded_config()
            .modes
            .get(self.input_state.current_mode())
            .map(|mode| mode.fallback_policy)
            .unwrap_or(FallbackPolicy::Ignore)
    }

    async fn focus_buffer(&mut self, session_id: SessionId, buffer_id: BufferId) -> Result<()> {
        let node_id = self
            .client
            .state()
            .buffers
            .get(&buffer_id)
            .and_then(|buffer| buffer.attachment_node_id)
            .ok_or_else(|| MuxError::invalid_input(format!("buffer {buffer_id} is detached")))?;

        let mut selections = Vec::new();
        let mut child_id = node_id;
        let mut parent_id = self
            .client
            .state()
            .nodes
            .get(&node_id)
            .and_then(|node| node.parent_id);
        while let Some(current_parent) = parent_id {
            if let Some(tabs) = self
                .client
                .state()
                .nodes
                .get(&current_parent)
                .and_then(|node| node.tabs.as_ref())
                && let Some(index) = tabs.tabs.iter().position(|tab| tab.child_id == child_id)
            {
                selections.push((current_parent, index));
            }
            child_id = current_parent;
            parent_id = self
                .client
                .state()
                .nodes
                .get(&current_parent)
                .and_then(|node| node.parent_id);
        }
        selections.reverse();

        for (tabs_node_id, index) in selections {
            self.client
                .request_message(ClientMessage::Node(NodeRequest::SelectTab {
                    request_id: self.client.next_request_id(),
                    tabs_node_id,
                    index: u32::try_from(index).map_err(|_| {
                        MuxError::invalid_input("tab index exceeds protocol limits")
                    })?,
                }))
                .await?;
        }
        self.client
            .request_message(ClientMessage::Node(NodeRequest::Focus {
                request_id: self.client.next_request_id(),
                session_id,
                node_id,
            }))
            .await?;
        self.client.resync_session(session_id).await
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

fn prepend_actions(pending: &mut VecDeque<Action>, actions: Vec<Action>) {
    for action in actions.into_iter().rev() {
        pending.push_front(action);
    }
}

fn format_notification(level: NotifyLevel, message: &str) -> String {
    match level {
        NotifyLevel::Info => message.to_owned(),
        NotifyLevel::Warn => format!("warn: {message}"),
        NotifyLevel::Error => format!("error: {message}"),
    }
}

fn resolve_floating_geometry(spec: FloatingGeometrySpec, viewport: Size) -> FloatGeometry {
    let width = resolve_floating_size(spec.width, viewport.width);
    let height = resolve_floating_size(spec.height, viewport.height);
    let max_x = viewport.width.saturating_sub(width);
    let max_y = viewport.height.saturating_sub(height);

    let (base_x, base_y) = match spec.anchor {
        FloatingAnchor::Center => (max_x / 2, max_y / 2),
        FloatingAnchor::TopLeft => (0, 0),
        FloatingAnchor::TopRight => (max_x, 0),
        FloatingAnchor::BottomLeft => (0, max_y),
        FloatingAnchor::BottomRight => (max_x, max_y),
    };

    let x = (i32::from(base_x) + i32::from(spec.offset_x)).clamp(0, i32::from(max_x));
    let y = (i32::from(base_y) + i32::from(spec.offset_y)).clamp(0, i32::from(max_y));

    FloatGeometry::new(x as u16, y as u16, width, height)
}

fn resolve_floating_size(size: FloatingSize, max: u16) -> u16 {
    match size {
        FloatingSize::Cells(cells) => cells.min(max.max(1)),
        FloatingSize::Percent(percent) => {
            let max = u32::from(max.max(1));
            let percent = u32::from(percent.max(1));
            ((max * percent) / 100).max(1) as u16
        }
    }
}

fn default_shell_command() -> Vec<String> {
    vec![std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned())]
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
        KeyEvent::Up => Ok(KeyToken::Up),
        KeyEvent::Down => Ok(KeyToken::Down),
        KeyEvent::Left => Ok(KeyToken::Left),
        KeyEvent::Right => Ok(KeyToken::Right),
        KeyEvent::PageUp => Ok(KeyToken::PageUp),
        KeyEvent::PageDown => Ok(KeyToken::PageDown),
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
            KeyToken::Up => bytes.extend_from_slice(b"\x1b[A"),
            KeyToken::Down => bytes.extend_from_slice(b"\x1b[B"),
            KeyToken::Left => bytes.extend_from_slice(b"\x1b[D"),
            KeyToken::Right => bytes.extend_from_slice(b"\x1b[C"),
            KeyToken::PageUp => bytes.extend_from_slice(b"\x1b[5~"),
            KeyToken::PageDown => bytes.extend_from_slice(b"\x1b[6~"),
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
        ServerEvent::SessionCreated(_) => "session_created",
        ServerEvent::SessionClosed(_) => "session_closed",
        ServerEvent::BufferCreated(_) => "buffer_created",
        ServerEvent::BufferDetached(_) => "buffer_detached",
        ServerEvent::NodeChanged(_) => "node_changed",
        ServerEvent::FloatingChanged(_) => "floating_changed",
        ServerEvent::FocusChanged(_) => "focus_changed",
        ServerEvent::RenderInvalidated(_) => "render_invalidated",
    }
}

fn event_info(name: &str, event: &ServerEvent) -> EventInfo {
    match event {
        ServerEvent::SessionCreated(event) => EventInfo {
            name: name.to_owned(),
            session_id: Some(event.session.id),
            buffer_id: None,
            node_id: None,
            floating_id: None,
        },
        ServerEvent::SessionClosed(event) => EventInfo {
            name: name.to_owned(),
            session_id: Some(event.session_id),
            buffer_id: None,
            node_id: None,
            floating_id: None,
        },
        ServerEvent::BufferCreated(event) => EventInfo {
            name: name.to_owned(),
            session_id: None,
            buffer_id: Some(event.buffer.id),
            node_id: event.buffer.attachment_node_id,
            floating_id: None,
        },
        ServerEvent::BufferDetached(event) => EventInfo {
            name: name.to_owned(),
            session_id: None,
            buffer_id: Some(event.buffer_id),
            node_id: None,
            floating_id: None,
        },
        ServerEvent::NodeChanged(event) => EventInfo {
            name: name.to_owned(),
            session_id: Some(event.session_id),
            buffer_id: None,
            node_id: None,
            floating_id: None,
        },
        ServerEvent::FloatingChanged(event) => EventInfo {
            name: name.to_owned(),
            session_id: Some(event.session_id),
            buffer_id: None,
            node_id: None,
            floating_id: None,
        },
        ServerEvent::FocusChanged(event) => EventInfo {
            name: name.to_owned(),
            session_id: Some(event.session_id),
            buffer_id: None,
            node_id: event.focused_leaf_id,
            floating_id: event.focused_floating_id,
        },
        ServerEvent::RenderInvalidated(event) => EventInfo {
            name: name.to_owned(),
            session_id: None,
            buffer_id: Some(event.buffer_id),
            node_id: None,
            floating_id: None,
        },
    }
}
