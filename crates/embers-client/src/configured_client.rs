use std::collections::{BTreeMap, VecDeque};
use std::path::Path;

use tracing::warn;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use embers_core::{
    ActivityState, BufferId, FloatGeometry, MuxError, NodeId, Point, Result, SessionId, Size,
};
use embers_protocol::{
    BufferLocation, BufferLocationAttachment, BufferRecord, BufferRequest, BufferResponse,
    ClientMessage, FloatingRequest, InputRequest, NodeRequest, ServerEvent, ServerResponse,
};

use crate::RenderGrid;
use crate::client::MuxClient;
use crate::config::ConfigManager;
use crate::controller::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use crate::input::{
    FallbackPolicy, InputResolution, InputState, KeyToken, NORMAL_MODE, SEARCH_MODE, SELECT_MODE,
    resolve_key,
};
use crate::presentation::{LeafFrame, NavigationDirection, PresentationModel};
use crate::renderer::Renderer;
use crate::scripting::{
    Action, BarSpec, Context, EventInfo, FloatingAnchor, FloatingGeometrySpec, FloatingSize,
    NotifyLevel, TabBarContext, TreeSpec,
};
use crate::state::{SearchMatch, SearchState, SelectionKind, SelectionPoint, SelectionState};
use crate::transport::Transport;

const WHEEL_SCROLL_LINES: u64 = 3;
const MAX_EXPANDED_ACTIONS: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
struct SearchPrompt {
    node_id: NodeId,
    query: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResolvedTreeBuffer {
    Existing(BufferId),
    NewlySpawned(BufferId),
}

impl ResolvedTreeBuffer {
    fn id(self) -> BufferId {
        match self {
            Self::Existing(buffer_id) | Self::NewlySpawned(buffer_id) => buffer_id,
        }
    }

    fn created_by_helper(self) -> Option<BufferId> {
        match self {
            Self::Existing(_) => None,
            Self::NewlySpawned(buffer_id) => Some(buffer_id),
        }
    }
}

pub struct ConfiguredClient<T> {
    client: MuxClient<T>,
    config: ConfigManager,
    input_state: InputState,
    renderer: Renderer,
    notifications: Vec<String>,
    active_session_id: Option<SessionId>,
    viewport: Option<Size>,
    search_prompt: Option<SearchPrompt>,
    terminal_output: VecDeque<Vec<u8>>,
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
            search_prompt: None,
            terminal_output: VecDeque::new(),
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

    pub fn drain_terminal_output(&mut self) -> Vec<Vec<u8>> {
        self.terminal_output.drain(..).collect()
    }

    pub fn status_line(&self, session_id: SessionId, socket_path: &Path) -> String {
        let session_name = self
            .client
            .state()
            .sessions
            .get(&session_id)
            .map(|session| session.name.as_str())
            .unwrap_or("<missing>");
        if let Some(prompt) = &self.search_prompt {
            return format!("[{session_name}] /{}", prompt.query);
        }
        match self.notifications.last() {
            Some(message) => format!("[{session_name}] {message}"),
            None => format!("[{session_name}] {}  ctrl-q quit", socket_path.display()),
        }
    }

    pub async fn handle_key(
        &mut self,
        session_id: SessionId,
        viewport: Size,
        key: KeyEvent,
    ) -> Result<()> {
        self.set_active_view(session_id, viewport);
        let presentation = self.prepare_presentation(session_id, viewport).await?;

        if self.input_state.current_mode() == SEARCH_MODE {
            return self
                .handle_search_key(session_id, viewport, &presentation, key)
                .await;
        }

        match key {
            KeyEvent::Bytes(bytes) => {
                if self.current_fallback_policy() != FallbackPolicy::Passthrough {
                    return Ok(());
                }
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
                        if self.should_passthrough_binding_in_alternate_screen(
                            &presentation,
                            &binding.target,
                        ) {
                            let buffer_id = self.resolve_buffer_id(None, &presentation)?;
                            return self
                                .send_bytes_to_buffer(
                                    buffer_id,
                                    session_id,
                                    sequence_to_bytes(&binding.sequence)?,
                                )
                                .await;
                        }
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
        if self.input_state.current_mode() == SEARCH_MODE {
            return self.handle_search_paste(session_id, viewport, bytes).await;
        }
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

    pub async fn handle_mouse(
        &mut self,
        session_id: SessionId,
        viewport: Size,
        event: MouseEvent,
    ) -> Result<()> {
        self.set_active_view(session_id, viewport);
        let presentation = self.prepare_presentation(session_id, viewport).await?;
        let point = Point {
            x: i32::from(event.column),
            y: i32::from(event.row),
        };
        let Some(target_leaf) = self.mouse_target_leaf(&presentation, point).cloned() else {
            return Ok(());
        };

        let settings = self.config.active_script().loaded_config().mouse;
        let target_snapshot = self.client.state().snapshots.get(&target_leaf.buffer_id);
        let mouse_reporting = target_snapshot.is_some_and(|snapshot| snapshot.mouse_reporting);
        let point_in_content = point.y > target_leaf.rect.origin.y;

        match event.kind {
            MouseEventKind::WheelUp | MouseEventKind::WheelDown => {
                if mouse_reporting && settings.wheel_forward && point_in_content {
                    return self
                        .send_bytes_to_buffer(
                            target_leaf.buffer_id,
                            session_id,
                            encode_mouse_event(&target_leaf, event)?,
                        )
                        .await;
                }
                if settings.wheel_scroll && !self.view_is_alternate_screen(target_leaf.node_id) {
                    let delta = match event.kind {
                        MouseEventKind::WheelUp => -(WHEEL_SCROLL_LINES as i64),
                        MouseEventKind::WheelDown => WHEEL_SCROLL_LINES as i64,
                        _ => 0,
                    };
                    return self.scroll_view_by(target_leaf.node_id, delta).await;
                }
                Ok(())
            }
            MouseEventKind::Press(_) | MouseEventKind::Release(_) | MouseEventKind::Drag(_) => {
                if settings.click_focus && !target_leaf.focused {
                    self.focus_node(session_id, target_leaf.node_id).await?;
                }
                if settings.click_forward && mouse_reporting && point_in_content {
                    self.send_bytes_to_buffer(
                        target_leaf.buffer_id,
                        session_id,
                        encode_mouse_event(&target_leaf, event)?,
                    )
                    .await?;
                }
                Ok(())
            }
        }
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
        let event = self.next_event().await?;
        self.handle_event(&event).await?;
        Ok(event)
    }

    pub async fn process_next_event_timeout(
        &mut self,
        timeout: std::time::Duration,
    ) -> Result<Option<ServerEvent>> {
        let Some(event) = tokio::time::timeout(timeout, self.client.next_event())
            .await
            .ok()
            .transpose()?
        else {
            return Ok(None);
        };
        self.handle_event(&event).await?;
        Ok(Some(event))
    }

    pub async fn next_event(&mut self) -> Result<ServerEvent> {
        self.client.next_event().await
    }

    pub async fn handle_event(&mut self, event: &ServerEvent) -> Result<()> {
        let previous_render_activity = match event {
            ServerEvent::RenderInvalidated(render) => self
                .client
                .state()
                .buffers
                .get(&render.buffer_id)
                .map(|buffer| buffer.activity),
            _ => None,
        };
        let detached_session_id = matches!(event, ServerEvent::BufferDetached(_))
            .then(|| self.event_session_id(event))
            .flatten();
        self.client.handle_event(event).await?;
        if let ServerEvent::RenderInvalidated(event) = event {
            self.client.refresh_buffer_snapshot(event.buffer_id).await?;
        }
        let session_id = detached_session_id.or_else(|| self.event_session_id(event));

        let mut event_names = vec![event_name(event).to_owned()];
        if let ServerEvent::RenderInvalidated(render) = event
            && previous_render_activity != Some(ActivityState::Bell)
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
                Some(event_info(&event_name, event, session_id)),
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
        Ok(())
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
        self.finish_config_reload(&current_mode);
        Ok(())
    }

    pub fn reload_config_if_changed(&mut self) -> Result<bool> {
        match self.config.reload_if_changed() {
            Ok(false) => Ok(false),
            Ok(true) => {
                let current_mode = self.input_state.current_mode().to_owned();
                self.finish_config_reload(&current_mode);
                Ok(true)
            }
            Err(error) => {
                let message = error.to_string();
                self.record_notification(message.clone());
                Err(MuxError::invalid_input(message))
            }
        }
    }

    fn finish_config_reload(&mut self, current_mode: &str) {
        if self
            .config
            .active_script()
            .loaded_config()
            .modes
            .contains_key(current_mode)
        {
            self.input_state.clear_pending();
        } else {
            self.input_state.set_mode(NORMAL_MODE);
            self.search_prompt = None;
        }
    }

    async fn execute_actions(
        &mut self,
        session_id: Option<SessionId>,
        viewport: Option<Size>,
        actions: Vec<Action>,
    ) -> Result<()> {
        let mut pending = VecDeque::from(actions);
        let mut expansions = 0usize;
        let mut current_session_id = session_id;
        let current_viewport = viewport;
        while let Some(action) = pending.pop_front() {
            let result = match action {
                Action::Noop => Ok(()),
                Action::Chain(actions) => {
                    prepend_actions_with_limit(&mut pending, actions, &mut expansions)
                }
                Action::RunNamedAction { name } => {
                    match self.config.active_script().run_named_action(
                        &name,
                        self.context_for(current_session_id, current_viewport, None),
                    ) {
                        Ok(actions) => {
                            prepend_actions_with_limit(&mut pending, actions, &mut expansions)
                        }
                        Err(error) => Err(MuxError::invalid_input(error.to_string())),
                    }
                }
                Action::EnterMode { mode } => {
                    let actions = self
                        .transition_mode(mode, current_session_id, current_viewport)
                        .await?;
                    prepend_actions_with_limit(&mut pending, actions, &mut expansions)
                }
                Action::LeaveMode => {
                    let actions = self
                        .transition_mode(
                            NORMAL_MODE.to_owned(),
                            current_session_id,
                            current_viewport,
                        )
                        .await?;
                    prepend_actions_with_limit(&mut pending, actions, &mut expansions)
                }
                Action::ToggleMode { mode } => {
                    let next_mode = if self.input_state.current_mode() == mode {
                        NORMAL_MODE.to_owned()
                    } else {
                        mode
                    };
                    let actions = self
                        .transition_mode(next_mode, current_session_id, current_viewport)
                        .await?;
                    prepend_actions_with_limit(&mut pending, actions, &mut expansions)
                }
                Action::ClearPendingKeys => {
                    self.input_state.clear_pending();
                    Ok(())
                }
                Action::Notify { level, message } => {
                    self.record_notification(format_notification(level, &message));
                    Ok(())
                }
                Action::FocusBuffer { buffer_id } => {
                    let location = Self::buffer_location_from_response(
                        self.client
                            .request_message(ClientMessage::Buffer(BufferRequest::GetLocation {
                                request_id: self.client.next_request_id(),
                                buffer_id,
                            }))
                            .await?,
                        "buffer focus",
                    )?;
                    let (session_id, node_id) =
                        Self::attached_buffer_location(Some(buffer_id), location, "buffer focus")?;
                    self.focus_node_with_shortcut(
                        session_id,
                        node_id,
                        self.active_session_id == Some(session_id),
                    )
                    .await?;
                    current_session_id = Some(session_id);
                    if let Some(viewport) = current_viewport {
                        self.set_active_view(session_id, viewport);
                    }
                    self.client.resync_all_sessions().await
                }
                Action::RevealBuffer { buffer_id } => {
                    let location = Self::buffer_location_from_response(
                        self.client
                            .request_message(ClientMessage::Buffer(BufferRequest::Reveal {
                                request_id: self.client.next_request_id(),
                                buffer_id,
                                client_id: None,
                            }))
                            .await?,
                        "buffer reveal",
                    )?;
                    let (session_id, _) =
                        Self::attached_buffer_location(Some(buffer_id), location, "buffer reveal")?;
                    current_session_id = Some(session_id);
                    if let Some(viewport) = current_viewport {
                        self.set_active_view(session_id, viewport);
                    }
                    self.client.resync_all_sessions().await
                }
                Action::OpenBufferHistory {
                    buffer_id,
                    scope,
                    placement,
                } => {
                    let location = Self::buffer_location_from_response(
                        self.client
                            .request_message(ClientMessage::Buffer(BufferRequest::OpenHistory {
                                request_id: self.client.next_request_id(),
                                buffer_id,
                                scope,
                                placement,
                                client_id: None,
                            }))
                            .await?,
                        "buffer history",
                    )?;
                    let (session_id, _) =
                        Self::attached_buffer_location(None, location, "buffer history")?;
                    current_session_id = Some(session_id);
                    if let Some(viewport) = current_viewport {
                        self.set_active_view(session_id, viewport);
                    }
                    self.client.resync_all_sessions().await
                }
                action => {
                    match self
                        .execute_without_presentation(current_session_id, action)
                        .await
                    {
                        Ok(None) => Ok(()),
                        Ok(Some(action)) => {
                            let mut missing = Vec::new();
                            if current_session_id.is_none() {
                                missing.push("current_session_id");
                            }
                            if current_viewport.is_none() {
                                missing.push("current_viewport");
                            }
                            if !missing.is_empty() {
                                return Err(MuxError::invalid_input(format!(
                                    "cannot execute action {action:?} without {}",
                                    missing.join(" and ")
                                )));
                            }
                            let session_id = current_session_id.expect("checked current session");
                            let viewport = current_viewport.expect("checked current viewport");
                            let presentation =
                                self.prepare_presentation(session_id, viewport).await?;
                            self.execute_action(session_id, viewport, &presentation, action)
                                .await
                        }
                        Err(error) => Err(error),
                    }
                }
            };
            if let Err(error) = result {
                self.record_notification(error.to_string());
            }
        }
        Ok(())
    }

    async fn execute_without_presentation(
        &mut self,
        current_session_id: Option<SessionId>,
        action: Action,
    ) -> Result<Option<Action>> {
        match action {
            Action::CloseFloating {
                floating_id: Some(floating_id),
            } => {
                self.client
                    .request_message(ClientMessage::Floating(FloatingRequest::Close {
                        request_id: self.client.next_request_id(),
                        floating_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await?;
                Ok(None)
            }
            Action::CloseView {
                node_id: Some(node_id),
            } => {
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::Close {
                        request_id: self.client.next_request_id(),
                        node_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await?;
                Ok(None)
            }
            Action::KillBuffer {
                buffer_id: Some(buffer_id),
            } => {
                self.client
                    .request_message(ClientMessage::Buffer(BufferRequest::Kill {
                        request_id: self.client.next_request_id(),
                        buffer_id,
                        force: false,
                    }))
                    .await?;
                self.client.resync_all_sessions().await?;
                Ok(None)
            }
            Action::DetachBuffer {
                buffer_id: Some(buffer_id),
            } => {
                self.client
                    .request_message(ClientMessage::Buffer(BufferRequest::Detach {
                        request_id: self.client.next_request_id(),
                        buffer_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await?;
                Ok(None)
            }
            Action::MoveBufferToNode { buffer_id, node_id } => {
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::MoveBufferToNode {
                        request_id: self.client.next_request_id(),
                        buffer_id,
                        target_leaf_node_id: node_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await?;
                Ok(None)
            }
            Action::ZoomNode {
                node_id: Some(node_id),
            } => {
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::Zoom {
                        request_id: self.client.next_request_id(),
                        node_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await?;
                Ok(None)
            }
            Action::UnzoomNode {
                session_id: target_session_id,
            } => {
                let Some(session_id) = target_session_id.or(current_session_id) else {
                    return Err(MuxError::invalid_input(format!(
                        "cannot execute action {:?} without current_session_id",
                        Action::UnzoomNode {
                            session_id: target_session_id
                        }
                    )));
                };
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::Unzoom {
                        request_id: self.client.next_request_id(),
                        session_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await?;
                Ok(None)
            }
            Action::ToggleZoomNode {
                node_id: Some(node_id),
            } => {
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::ToggleZoom {
                        request_id: self.client.next_request_id(),
                        node_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await?;
                Ok(None)
            }
            Action::SwapSiblingNodes {
                first_node_id: Some(first_node_id),
                second_node_id,
            } => {
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::SwapSiblings {
                        request_id: self.client.next_request_id(),
                        first_node_id,
                        second_node_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await?;
                Ok(None)
            }
            Action::BreakNode {
                node_id: Some(node_id),
                destination,
            } => {
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::BreakNode {
                        request_id: self.client.next_request_id(),
                        node_id,
                        destination,
                    }))
                    .await?;
                self.client.resync_all_sessions().await?;
                Ok(None)
            }
            Action::JoinBufferAtNode {
                node_id: Some(node_id),
                buffer_id,
                placement,
            } => {
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::JoinBufferAtNode {
                        request_id: self.client.next_request_id(),
                        node_id,
                        buffer_id,
                        placement,
                    }))
                    .await?;
                self.client.resync_all_sessions().await?;
                Ok(None)
            }
            Action::MoveNodeBefore {
                node_id: Some(node_id),
                sibling_node_id,
            } => {
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::MoveNodeBefore {
                        request_id: self.client.next_request_id(),
                        node_id,
                        sibling_node_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await?;
                Ok(None)
            }
            Action::MoveNodeAfter {
                node_id: Some(node_id),
                sibling_node_id,
            } => {
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::MoveNodeAfter {
                        request_id: self.client.next_request_id(),
                        node_id,
                        sibling_node_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await?;
                Ok(None)
            }
            other => Ok(Some(other)),
        }
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
                .map_err(|error| {
                    self.input_state.set_mode(previous_mode.clone());
                    MuxError::invalid_input(error.to_string())
                })?,
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
                self.focus_node(session_id, node_id).await
            }
            Action::ScrollLineUp => {
                let leaf = self.focused_leaf(presentation)?;
                self.scroll_view_by(leaf.node_id, -1).await
            }
            Action::ScrollLineDown => {
                let leaf = self.focused_leaf(presentation)?;
                self.scroll_view_by(leaf.node_id, 1).await
            }
            Action::ScrollPageUp => {
                let leaf = self.focused_leaf(presentation)?;
                let page = self
                    .client
                    .state()
                    .view_state(leaf.node_id)
                    .map(|state| i64::from(state.visible_line_count.max(1)))
                    .unwrap_or(1);
                self.scroll_view_by(leaf.node_id, -page).await
            }
            Action::ScrollPageDown => {
                let leaf = self.focused_leaf(presentation)?;
                let page = self
                    .client
                    .state()
                    .view_state(leaf.node_id)
                    .map(|state| i64::from(state.visible_line_count.max(1)))
                    .unwrap_or(1);
                self.scroll_view_by(leaf.node_id, page).await
            }
            Action::ScrollToTop => {
                let leaf = self.focused_leaf(presentation)?;
                self.set_view_scroll_top(leaf.node_id, 0).await
            }
            Action::ScrollToBottom | Action::FollowOutput => {
                let leaf = self.focused_leaf(presentation)?;
                self.follow_output_for_view(leaf.node_id).await
            }
            Action::EnterSearchMode => {
                self.enter_search_mode(session_id, viewport, presentation)
                    .await
            }
            Action::SearchNext => self.navigate_search(presentation, true).await,
            Action::SearchPrev => self.navigate_search(presentation, false).await,
            Action::CommitSearch => self.commit_search_prompt(session_id, viewport).await,
            Action::CancelSearch => self.cancel_search_prompt(session_id, viewport).await,
            Action::EnterSelect { kind } => {
                self.enter_select_mode(session_id, viewport, presentation, kind)
                    .await
            }
            Action::SelectMove { direction } => {
                let leaf = self.focused_leaf(presentation)?;
                self.move_selection(leaf.node_id, direction).await
            }
            Action::CopySelection => {
                let leaf = self.focused_leaf(presentation)?;
                self.copy_selection(session_id, viewport, leaf.node_id)
                    .await
            }
            Action::CancelSelection => {
                let leaf = self.focused_leaf(presentation)?;
                self.cancel_selection(session_id, viewport, leaf.node_id)
                    .await
            }
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
                let buffer = self
                    .resolve_tree_buffer(session_id, presentation, new_child)
                    .await?;
                let result = self
                    .client
                    .request_message(ClientMessage::Node(NodeRequest::Split {
                        request_id: self.client.next_request_id(),
                        leaf_node_id: focused_leaf.node_id,
                        direction,
                        new_buffer_id: buffer.id(),
                    }))
                    .await;
                rollback_created_buffer_on_error(
                    self,
                    buffer.created_by_helper(),
                    "split pane",
                    result,
                )
                .await?;
                self.client.resync_all_sessions().await
            }
            Action::OpenFloating { spec } => {
                let buffer = self
                    .resolve_tree_buffer(session_id, presentation, spec.tree)
                    .await?;
                let result = self
                    .client
                    .request_message(ClientMessage::Floating(FloatingRequest::Create {
                        request_id: self.client.next_request_id(),
                        session_id,
                        root_node_id: None,
                        buffer_id: Some(buffer.id()),
                        geometry: resolve_floating_geometry(spec.geometry, viewport),
                        title: spec.title,
                        focus: spec.focus,
                        close_on_empty: spec.close_on_empty,
                    }))
                    .await;
                rollback_created_buffer_on_error(
                    self,
                    buffer.created_by_helper(),
                    "open floating window",
                    result,
                )
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
                let buffer = self
                    .resolve_tree_buffer(session_id, presentation, child)
                    .await?;
                let result = self
                    .client
                    .request_message(ClientMessage::Node(NodeRequest::AddTab {
                        request_id: self.client.next_request_id(),
                        tabs_node_id: tabs.node_id,
                        title: title.unwrap_or_else(|| "tab".to_owned()),
                        buffer_id: Some(buffer.id()),
                        child_node_id: None,
                        index: u32::try_from(tabs.active.saturating_add(1)).map_err(|_| {
                            MuxError::invalid_input("tab index exceeds protocol limits")
                        })?,
                    }))
                    .await;
                rollback_created_buffer_on_error(
                    self,
                    buffer.created_by_helper(),
                    "insert tab",
                    result,
                )
                .await?;
                self.client.resync_all_sessions().await
            }
            Action::InsertTabBefore {
                tabs_node_id,
                title,
                child,
            } => {
                let tabs = self.resolve_tabs_target(presentation, tabs_node_id)?;
                let buffer = self
                    .resolve_tree_buffer(session_id, presentation, child)
                    .await?;
                let result = self
                    .client
                    .request_message(ClientMessage::Node(NodeRequest::AddTab {
                        request_id: self.client.next_request_id(),
                        tabs_node_id: tabs.node_id,
                        title: title.unwrap_or_else(|| "tab".to_owned()),
                        buffer_id: Some(buffer.id()),
                        child_node_id: None,
                        index: u32::try_from(tabs.active).map_err(|_| {
                            MuxError::invalid_input("tab index exceeds protocol limits")
                        })?,
                    }))
                    .await;
                rollback_created_buffer_on_error(
                    self,
                    buffer.created_by_helper(),
                    "insert tab",
                    result,
                )
                .await?;
                self.client.resync_all_sessions().await
            }
            Action::ReplaceNode { node_id, tree } => {
                let node_id = node_id
                    .or_else(|| presentation.focused_leaf().map(|leaf| leaf.node_id))
                    .ok_or_else(|| MuxError::invalid_input("no focused node to replace"))?;
                let buffer = self
                    .resolve_tree_buffer(session_id, presentation, tree)
                    .await?;
                let result = self
                    .client
                    .request_message(ClientMessage::Node(NodeRequest::MoveBufferToNode {
                        request_id: self.client.next_request_id(),
                        buffer_id: buffer.id(),
                        target_leaf_node_id: node_id,
                    }))
                    .await;
                rollback_created_buffer_on_error(
                    self,
                    buffer.created_by_helper(),
                    "replace node buffer",
                    result,
                )
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
                close_on_empty,
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
                        close_on_empty,
                    }))
                    .await?;
                self.client.resync_all_sessions().await
            }
            Action::ZoomNode { node_id: None } => {
                let node_id = presentation
                    .focused_leaf()
                    .map(|leaf| leaf.node_id)
                    .ok_or_else(|| MuxError::invalid_input("no focused node to zoom"))?;
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::Zoom {
                        request_id: self.client.next_request_id(),
                        node_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await
            }
            Action::ToggleZoomNode { node_id } => {
                let node_id = node_id
                    .or_else(|| presentation.focused_leaf().map(|leaf| leaf.node_id))
                    .ok_or_else(|| MuxError::invalid_input("no focused node to toggle zoom"))?;
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::ToggleZoom {
                        request_id: self.client.next_request_id(),
                        node_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await
            }
            Action::SwapSiblingNodes {
                first_node_id,
                second_node_id,
            } => {
                let first_node_id = first_node_id
                    .or_else(|| presentation.focused_leaf().map(|leaf| leaf.node_id))
                    .ok_or_else(|| MuxError::invalid_input("no focused node to swap"))?;
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::SwapSiblings {
                        request_id: self.client.next_request_id(),
                        first_node_id,
                        second_node_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await
            }
            Action::BreakNode {
                node_id,
                destination,
            } => {
                let node_id = node_id
                    .or_else(|| presentation.focused_leaf().map(|leaf| leaf.node_id))
                    .ok_or_else(|| MuxError::invalid_input("no focused node to break"))?;
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::BreakNode {
                        request_id: self.client.next_request_id(),
                        node_id,
                        destination,
                    }))
                    .await?;
                self.client.resync_all_sessions().await
            }
            Action::JoinBufferAtNode {
                node_id,
                buffer_id,
                placement,
            } => {
                let node_id = node_id
                    .or_else(|| presentation.focused_leaf().map(|leaf| leaf.node_id))
                    .ok_or_else(|| {
                        MuxError::invalid_input("no focused node to join buffer into")
                    })?;
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::JoinBufferAtNode {
                        request_id: self.client.next_request_id(),
                        node_id,
                        buffer_id,
                        placement,
                    }))
                    .await?;
                self.client.resync_all_sessions().await
            }
            Action::MoveNodeBefore {
                node_id,
                sibling_node_id,
            } => {
                let node_id = node_id
                    .or_else(|| presentation.focused_leaf().map(|leaf| leaf.node_id))
                    .ok_or_else(|| MuxError::invalid_input("no focused node to reorder"))?;
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::MoveNodeBefore {
                        request_id: self.client.next_request_id(),
                        node_id,
                        sibling_node_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await
            }
            Action::MoveNodeAfter {
                node_id,
                sibling_node_id,
            } => {
                let node_id = node_id
                    .or_else(|| presentation.focused_leaf().map(|leaf| leaf.node_id))
                    .ok_or_else(|| MuxError::invalid_input("no focused node to reorder"))?;
                self.client
                    .request_message(ClientMessage::Node(NodeRequest::MoveNodeAfter {
                        request_id: self.client.next_request_id(),
                        node_id,
                        sibling_node_id,
                    }))
                    .await?;
                self.client.resync_all_sessions().await
            }
            Action::FocusBuffer { .. }
            | Action::RevealBuffer { .. }
            | Action::OpenBufferHistory { .. }
            | Action::UnzoomNode { .. }
            | Action::ZoomNode { node_id: Some(_) } => Err(MuxError::invalid_input(
                "action should be handled before presentation is required",
            )),
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
    ) -> Result<ResolvedTreeBuffer> {
        match tree {
            TreeSpec::BufferCurrent => presentation
                .focused_buffer_id()
                .map(ResolvedTreeBuffer::Existing)
                .ok_or_else(|| MuxError::invalid_input("no current buffer is focused")),
            TreeSpec::BufferAttach { buffer_id } => Ok(ResolvedTreeBuffer::Existing(buffer_id)),
            TreeSpec::BufferSpawn(spec) => self
                .create_buffer(spec)
                .await
                .map(ResolvedTreeBuffer::NewlySpawned),
            TreeSpec::BufferEmpty => self
                .create_buffer(crate::scripting::BufferSpawnSpec {
                    title: Some("shell".to_owned()),
                    command: default_shell_command(),
                    cwd: None,
                    env: Default::default(),
                })
                .await
                .map(ResolvedTreeBuffer::NewlySpawned),
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

    async fn rollback_created_buffer(&mut self, buffer_id: BufferId, operation: &str) {
        if let Err(error) = self
            .client
            .request_message(ClientMessage::Buffer(BufferRequest::Detach {
                request_id: self.client.next_request_id(),
                buffer_id,
            }))
            .await
        {
            warn!(
                %buffer_id,
                %error,
                operation,
                "failed to detach created buffer during rollback"
            );
        }
        if let Err(error) = self
            .client
            .request_message(ClientMessage::Buffer(BufferRequest::Kill {
                request_id: self.client.next_request_id(),
                buffer_id,
                force: true,
            }))
            .await
        {
            warn!(
                %buffer_id,
                %error,
                operation,
                "failed to kill created buffer during rollback"
            );
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
                                        buffer_count: crate::presentation::subtree_buffer_count(
                                            self.client.state(),
                                            tab.child_id,
                                        ),
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
        self.refresh_local_viewports(&presentation).await?;
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
                None,
                None,
                None,
                None,
            )
        } else {
            Context::from_state_with_mode(
                self.client.state(),
                None,
                self.input_state.current_mode(),
                session_id,
                None,
                None,
                None,
            )
        };
        if let Some(event) = event {
            context.with_event(event)
        } else {
            context
        }
    }

    fn event_session_id(&self, event: &ServerEvent) -> Option<SessionId> {
        event.session_id().or_else(|| match event {
            ServerEvent::BufferCreated(event) => self.session_id_for_buffer_record(&event.buffer),
            ServerEvent::BufferDetached(event) => self.session_id_for_buffer(event.buffer_id),
            ServerEvent::RenderInvalidated(event) => self.session_id_for_buffer(event.buffer_id),
            ServerEvent::SessionCreated(_)
            | ServerEvent::SessionClosed(_)
            | ServerEvent::SessionRenamed(_)
            | ServerEvent::NodeChanged(_)
            | ServerEvent::FloatingChanged(_)
            | ServerEvent::FocusChanged(_) => None,
            ServerEvent::ClientChanged(event) => event.client.current_session_id,
        })
    }

    fn session_id_for_buffer_record(&self, buffer: &BufferRecord) -> Option<SessionId> {
        buffer
            .attachment_node_id
            .and_then(|node_id| self.session_id_for_node(node_id))
            .or_else(|| self.session_id_for_buffer(buffer.id))
    }

    fn session_id_for_buffer(&self, buffer_id: BufferId) -> Option<SessionId> {
        let state = self.client.state();
        state
            .buffers
            .get(&buffer_id)
            .and_then(|buffer| buffer.attachment_node_id)
            .and_then(|node_id| state.nodes.get(&node_id))
            .map(|node| node.session_id)
            .or_else(|| {
                state.nodes.values().find_map(|node| {
                    node.buffer_view
                        .as_ref()
                        .filter(|view| view.buffer_id == buffer_id)
                        .map(|_| node.session_id)
                })
            })
    }

    fn session_id_for_node(&self, node_id: NodeId) -> Option<SessionId> {
        self.client
            .state()
            .nodes
            .get(&node_id)
            .map(|node| node.session_id)
    }

    fn buffer_location_from_response(
        response: ServerResponse,
        context: &str,
    ) -> Result<BufferLocation> {
        match response {
            ServerResponse::BufferLocation(response) => Ok(response.location),
            ServerResponse::BufferWithLocation(response) => {
                let (_, _, location, _) = response.into_parts();
                Ok(location)
            }
            other => Err(MuxError::protocol(format!(
                "expected {context} response, got {other:?}"
            ))),
        }
    }

    fn attached_buffer_location(
        expected_buffer_id: Option<BufferId>,
        location: BufferLocation,
        context: &str,
    ) -> Result<(SessionId, NodeId)> {
        if let Some(expected_buffer_id) = expected_buffer_id
            && location.buffer_id != expected_buffer_id
        {
            return Err(MuxError::protocol(format!(
                "{context} returned location for buffer {} while acting on buffer {expected_buffer_id}",
                location.buffer_id
            )));
        }

        match location.attachment {
            BufferLocationAttachment::Session {
                session_id,
                node_id,
            }
            | BufferLocationAttachment::Floating {
                session_id,
                node_id,
                ..
            } => Ok((session_id, node_id)),
            BufferLocationAttachment::Detached => Err(MuxError::conflict(format!(
                "{context} returned a detached location for buffer {}",
                expected_buffer_id.unwrap_or(location.buffer_id)
            ))),
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

    fn focused_leaf<'a>(&self, presentation: &'a PresentationModel) -> Result<&'a LeafFrame> {
        presentation
            .focused_leaf()
            .ok_or_else(|| MuxError::invalid_input("no focused leaf"))
    }

    fn mouse_target_leaf<'a>(
        &self,
        presentation: &'a PresentationModel,
        point: Point,
    ) -> Option<&'a LeafFrame> {
        if let Some(floating) = presentation.floating_at(point) {
            return presentation.leaves.iter().rev().find(|leaf| {
                leaf.floating_id == Some(floating.floating_id) && leaf.rect.contains(point)
            });
        }
        presentation.leaf_at(point)
    }

    fn view_is_alternate_screen(&self, node_id: NodeId) -> bool {
        self.client
            .state()
            .view_state(node_id)
            .is_some_and(|state| state.alternate_screen)
    }

    fn should_passthrough_binding_in_alternate_screen(
        &self,
        presentation: &PresentationModel,
        actions: &[Action],
    ) -> bool {
        // When the focused pane is acting like a live terminal surface, prefer
        // forwarding local search/select/navigation bindings to the program
        // instead of stealing keys that fullscreen apps expect to receive.
        let Some(leaf) = presentation.focused_leaf() else {
            return false;
        };
        let Some(view_state) = self.client.state().view_state(leaf.node_id) else {
            return false;
        };
        (view_state.alternate_screen && actions.iter().all(action_is_local_terminal_action))
            || (self.input_state.current_mode() == NORMAL_MODE
                && view_state.follow_output
                && view_state.search_state.is_none()
                && view_state.selection_state.is_none()
                && actions.iter().all(action_requires_local_context))
    }

    async fn refresh_local_viewports(&mut self, presentation: &PresentationModel) -> Result<()> {
        let refreshes = presentation
            .leaves
            .iter()
            .filter_map(|leaf| {
                let state = self.client.state().view_state(leaf.node_id)?;
                if state.alternate_screen
                    || state.follow_output
                    || state.visible_line_count == 0
                    || !state.visible_lines.is_empty()
                {
                    return None;
                }
                Some((leaf.node_id, state.scroll_top_line))
            })
            .collect::<Vec<_>>();

        for (node_id, scroll_top_line) in refreshes {
            self.fetch_view_slice(node_id, scroll_top_line).await?;
        }
        Ok(())
    }

    async fn scroll_view_by(&mut self, node_id: NodeId, delta: i64) -> Result<()> {
        let Some(state) = self.client.state().view_state(node_id) else {
            return Ok(());
        };
        if state.alternate_screen {
            return Ok(());
        }
        let current = i128::from(state.scroll_top_line);
        let delta = i128::from(delta);
        // Negative deltas scroll up by subtracting the absolute value; positive deltas
        // scroll down by adding. We do the math in i128 with saturating ops so
        // unsigned_abs/subtraction cannot underflow or overflow before clamping at 0.
        let next = if delta.is_negative() {
            current.saturating_sub(delta.unsigned_abs() as i128)
        } else {
            current.saturating_add(delta)
        };
        let next = next.max(0) as u64;
        self.set_view_scroll_top(node_id, next).await
    }

    async fn set_view_scroll_top(&mut self, node_id: NodeId, scroll_top_line: u64) -> Result<()> {
        let Some(state) = self.client.state().view_state(node_id) else {
            return Ok(());
        };
        if state.alternate_screen {
            return Ok(());
        }
        let bottom = state
            .total_line_count
            .saturating_sub(u64::from(state.visible_line_count));
        let scroll_top_line = scroll_top_line.min(bottom);
        if scroll_top_line == bottom {
            return self.follow_output_for_view(node_id).await;
        }

        self.fetch_view_slice(node_id, scroll_top_line).await?;
        if let Some(state) = self.client.state_mut().view_state_mut(node_id) {
            state.follow_output = false;
        }
        Ok(())
    }

    async fn follow_output_for_view(&mut self, node_id: NodeId) -> Result<()> {
        let Some(state) = self.client.state().view_state(node_id) else {
            return Ok(());
        };
        if state.alternate_screen {
            return Ok(());
        }
        self.client
            .state_mut()
            .set_view_follow_output(node_id, true);
        Ok(())
    }

    async fn fetch_view_slice(&mut self, node_id: NodeId, scroll_top_line: u64) -> Result<()> {
        let Some(state) = self.client.state().view_state(node_id) else {
            return Ok(());
        };
        if state.visible_line_count == 0 {
            return Ok(());
        }
        let response = self
            .client
            .capture_scrollback_slice(
                state.buffer_id,
                scroll_top_line,
                u32::from(state.visible_line_count),
            )
            .await?;
        if let Some(state) = self.client.state_mut().view_state_mut(node_id) {
            state.total_line_count = response
                .total_lines
                .max(u64::from(state.visible_line_count));
        }
        self.client
            .state_mut()
            .set_view_visible_lines(node_id, scroll_top_line, response.lines);
        Ok(())
    }

    async fn enter_search_mode(
        &mut self,
        _session_id: SessionId,
        _viewport: Size,
        presentation: &PresentationModel,
    ) -> Result<()> {
        let leaf = self.focused_leaf(presentation)?;
        if self.view_is_alternate_screen(leaf.node_id) {
            return Ok(());
        }

        let query = self
            .client
            .state()
            .view_state(leaf.node_id)
            .and_then(|state| state.search_state.as_ref())
            .map(|state| state.query.clone())
            .unwrap_or_default();
        self.search_prompt = Some(SearchPrompt {
            node_id: leaf.node_id,
            query,
        });
        self.input_state.set_mode(SEARCH_MODE);
        Ok(())
    }

    async fn handle_search_key(
        &mut self,
        session_id: SessionId,
        viewport: Size,
        presentation: &PresentationModel,
        key: KeyEvent,
    ) -> Result<()> {
        if self.search_prompt.is_none() {
            return self
                .enter_search_mode(session_id, viewport, presentation)
                .await;
        }

        match key {
            KeyEvent::Char(ch) => {
                if let Some(prompt) = &mut self.search_prompt {
                    prompt.query.push(ch);
                }
                Ok(())
            }
            KeyEvent::Tab => {
                if let Some(prompt) = &mut self.search_prompt {
                    prompt.query.push('\t');
                }
                Ok(())
            }
            KeyEvent::Backspace => {
                if let Some(prompt) = &mut self.search_prompt {
                    prompt.query.pop();
                }
                Ok(())
            }
            KeyEvent::Enter => {
                self.execute_actions(Some(session_id), Some(viewport), vec![Action::CommitSearch])
                    .await
            }
            KeyEvent::Escape => {
                self.execute_actions(Some(session_id), Some(viewport), vec![Action::CancelSearch])
                    .await
            }
            KeyEvent::Bytes(bytes) => {
                if let Some(prompt) = &mut self.search_prompt {
                    prompt.query.push_str(&String::from_utf8_lossy(&bytes));
                }
                Ok(())
            }
            KeyEvent::Ctrl(_)
            | KeyEvent::Alt(_)
            | KeyEvent::Up
            | KeyEvent::Down
            | KeyEvent::Left
            | KeyEvent::Right
            | KeyEvent::Home
            | KeyEvent::End
            | KeyEvent::Insert
            | KeyEvent::Delete
            | KeyEvent::PageUp
            | KeyEvent::PageDown => Ok(()),
        }
    }

    async fn handle_search_paste(
        &mut self,
        session_id: SessionId,
        viewport: Size,
        bytes: Vec<u8>,
    ) -> Result<()> {
        if self.search_prompt.is_none() {
            return self.cancel_search_prompt(session_id, viewport).await;
        }
        if let Some(prompt) = &mut self.search_prompt {
            prompt.query.push_str(&String::from_utf8_lossy(&bytes));
        }
        Ok(())
    }

    async fn commit_search_prompt(&mut self, session_id: SessionId, viewport: Size) -> Result<()> {
        let Some(prompt) = self.search_prompt.take() else {
            return self.cancel_search_prompt(session_id, viewport).await;
        };
        let Some(buffer_id) = self
            .client
            .state()
            .view_state(prompt.node_id)
            .map(|state| state.buffer_id)
        else {
            return self.cancel_search_prompt(session_id, viewport).await;
        };

        let matches = if prompt.query.is_empty() {
            Vec::new()
        } else {
            let snapshot = self.client.capture_buffer(buffer_id).await?;
            compute_search_matches(&snapshot.lines, &prompt.query)
        };

        if let Some(state) = self.client.state_mut().view_state_mut(prompt.node_id) {
            if prompt.query.is_empty() {
                state.search_state = None;
            } else {
                state.search_state = Some(SearchState {
                    query: prompt.query.clone(),
                    matches,
                    active_match_index: None,
                });
            }
        }

        if self
            .client
            .state()
            .view_state(prompt.node_id)
            .and_then(|state| state.search_state.as_ref())
            .is_some_and(|state| !state.matches.is_empty())
        {
            self.jump_to_search_index(prompt.node_id, 0).await?;
        }
        self.input_state.set_mode(NORMAL_MODE);
        Ok(())
    }

    async fn cancel_search_prompt(
        &mut self,
        _session_id: SessionId,
        _viewport: Size,
    ) -> Result<()> {
        self.search_prompt = None;
        self.input_state.set_mode(NORMAL_MODE);
        Ok(())
    }

    async fn navigate_search(
        &mut self,
        presentation: &PresentationModel,
        forward: bool,
    ) -> Result<()> {
        let leaf = self.focused_leaf(presentation)?;
        if self.view_is_alternate_screen(leaf.node_id) {
            return Ok(());
        }
        let Some(search_state) = self
            .client
            .state()
            .view_state(leaf.node_id)
            .and_then(|state| state.search_state.as_ref())
        else {
            return Ok(());
        };
        if search_state.matches.is_empty() {
            return Ok(());
        }
        let current = search_state.active_match_index.unwrap_or(0);
        let next = if forward {
            (current + 1) % search_state.matches.len()
        } else if current == 0 {
            search_state.matches.len() - 1
        } else {
            current - 1
        };
        self.jump_to_search_index(leaf.node_id, next).await
    }

    async fn jump_to_search_index(&mut self, node_id: NodeId, index: usize) -> Result<()> {
        let Some((selected, current_top, visible_line_count)) =
            self.client.state().view_state(node_id).and_then(|state| {
                state.search_state.as_ref().and_then(|search| {
                    search
                        .matches
                        .get(index)
                        .copied()
                        .map(|selected| (selected, state.scroll_top_line, state.visible_line_count))
                })
            })
        else {
            return Ok(());
        };

        let visible_line_count = u64::from(visible_line_count.max(1));
        let new_top = if selected.line < current_top {
            selected.line
        } else if selected.line >= current_top.saturating_add(visible_line_count) {
            selected
                .line
                .saturating_sub(visible_line_count.saturating_sub(1))
        } else {
            current_top
        };
        if new_top != current_top
            || self
                .client
                .state()
                .view_state(node_id)
                .is_some_and(|state| state.follow_output)
        {
            self.fetch_view_slice(node_id, new_top).await?;
        }

        if let Some(state) = self.client.state_mut().view_state_mut(node_id) {
            if let Some(search) = &mut state.search_state {
                search.active_match_index = Some(index);
            }
            state.follow_output = false;
        }
        Ok(())
    }

    async fn enter_select_mode(
        &mut self,
        _session_id: SessionId,
        _viewport: Size,
        presentation: &PresentationModel,
        kind: SelectionKind,
    ) -> Result<()> {
        let leaf = self.focused_leaf(presentation)?;
        if self.view_is_alternate_screen(leaf.node_id) {
            return Ok(());
        }

        let start = self.selection_origin(leaf.node_id);
        if let Some(state) = self.client.state_mut().view_state_mut(leaf.node_id) {
            state.selection_state = Some(SelectionState {
                kind,
                anchor: start,
                cursor: start,
            });
        }
        self.input_state.set_mode(SELECT_MODE);
        Ok(())
    }

    fn selection_origin(&self, node_id: NodeId) -> SelectionPoint {
        let Some(state) = self.client.state().view_state(node_id) else {
            return SelectionPoint::default();
        };
        let fallback = SelectionPoint {
            line: state.scroll_top_line,
            column: 0,
        };
        if !state.follow_output {
            return fallback;
        }
        self.client
            .state()
            .snapshots
            .get(&state.buffer_id)
            .and_then(|snapshot| {
                snapshot.cursor.map(|cursor| SelectionPoint {
                    line: state.scroll_top_line + u64::from(cursor.position.row),
                    column: cursor.position.col,
                })
            })
            .unwrap_or(fallback)
    }

    async fn move_selection(
        &mut self,
        node_id: NodeId,
        direction: NavigationDirection,
    ) -> Result<()> {
        let Some((mut selection, total_line_count, scroll_top_line, visible_line_count)) =
            self.client.state().view_state(node_id).and_then(|state| {
                state.selection_state.clone().map(|selection| {
                    (
                        selection,
                        state.total_line_count,
                        state.scroll_top_line,
                        state.visible_line_count,
                    )
                })
            })
        else {
            return Ok(());
        };

        match direction {
            NavigationDirection::Left => {
                selection.cursor.column = selection.cursor.column.saturating_sub(1);
            }
            NavigationDirection::Right => {
                selection.cursor.column = selection.cursor.column.saturating_add(1);
            }
            NavigationDirection::Up => {
                selection.cursor.line = selection.cursor.line.saturating_sub(1);
            }
            NavigationDirection::Down => {
                selection.cursor.line = selection
                    .cursor
                    .line
                    .saturating_add(1)
                    .min(total_line_count.saturating_sub(1));
            }
        }

        let visible_line_count = u64::from(visible_line_count.max(1));
        let mut new_top = scroll_top_line;
        if selection.cursor.line < new_top {
            new_top = selection.cursor.line;
        } else if selection.cursor.line >= new_top.saturating_add(visible_line_count) {
            new_top = selection
                .cursor
                .line
                .saturating_sub(visible_line_count.saturating_sub(1));
        }
        if new_top != scroll_top_line {
            self.fetch_view_slice(node_id, new_top).await?;
        }

        if let Some(state) = self.client.state_mut().view_state_mut(node_id) {
            state.follow_output = false;
            state.selection_state = Some(selection);
        }
        Ok(())
    }

    async fn copy_selection(
        &mut self,
        _session_id: SessionId,
        _viewport: Size,
        node_id: NodeId,
    ) -> Result<()> {
        let Some((buffer_id, selection_state)) =
            self.client.state().view_state(node_id).and_then(|state| {
                state
                    .selection_state
                    .clone()
                    .map(|selection| (state.buffer_id, selection))
            })
        else {
            return Ok(());
        };

        let snapshot = self.client.capture_buffer(buffer_id).await?;
        let copied = serialize_selection(&snapshot.lines, &selection_state);
        self.enqueue_clipboard(copied);

        if let Some(state) = self.client.state_mut().view_state_mut(node_id) {
            state.selection_state = None;
        }
        self.input_state.set_mode(NORMAL_MODE);
        Ok(())
    }

    async fn cancel_selection(
        &mut self,
        _session_id: SessionId,
        _viewport: Size,
        node_id: NodeId,
    ) -> Result<()> {
        if let Some(state) = self.client.state_mut().view_state_mut(node_id) {
            state.selection_state = None;
        }
        self.input_state.set_mode(NORMAL_MODE);
        Ok(())
    }

    fn enqueue_clipboard(&mut self, text: String) {
        use base64::Engine as _;

        let encoded = base64::engine::general_purpose::STANDARD.encode(text);
        self.terminal_output
            .push_back(format!("\x1b]52;c;{encoded}\x07").into_bytes());
    }

    async fn focus_node(&mut self, session_id: SessionId, node_id: NodeId) -> Result<()> {
        self.focus_node_with_shortcut(session_id, node_id, true)
            .await
    }

    async fn focus_node_with_shortcut(
        &mut self,
        session_id: SessionId,
        node_id: NodeId,
        allow_same_session_shortcut: bool,
    ) -> Result<()> {
        if allow_same_session_shortcut
            && self
                .client
                .state()
                .sessions
                .get(&session_id)
                .and_then(|session| session.focused_leaf_id)
                == Some(node_id)
        {
            return Ok(());
        }

        let previous_buffer = self
            .active_session_id
            .and_then(|active_session_id| self.focused_buffer_for_session(active_session_id));
        let sent_focus_out = if let Some(buffer_id) = previous_buffer {
            self.maybe_send_focus_sequence(buffer_id, false).await?
        } else {
            false
        };

        self.client
            .request_message(ClientMessage::Node(NodeRequest::Focus {
                request_id: self.client.next_request_id(),
                session_id,
                node_id,
            }))
            .await?;
        self.client.resync_session(session_id).await?;

        let new_buffer = self.focused_buffer_for_session(session_id);
        if new_buffer != previous_buffer {
            let sent_focus_in = if let Some(buffer_id) = new_buffer {
                self.maybe_send_focus_sequence(buffer_id, true).await?
            } else {
                false
            };
            if sent_focus_in && let Some(buffer_id) = new_buffer {
                self.client.refresh_buffer_snapshot(buffer_id).await?;
            }
            if sent_focus_out && let Some(buffer_id) = previous_buffer {
                self.client.refresh_buffer_snapshot(buffer_id).await?;
            }
        }
        Ok(())
    }

    fn focused_buffer_for_session(&self, session_id: SessionId) -> Option<BufferId> {
        self.client
            .state()
            .sessions
            .get(&session_id)
            .and_then(|session| session.focused_leaf_id)
            .and_then(|node_id| self.node_buffer_id(node_id))
    }

    fn node_buffer_id(&self, node_id: NodeId) -> Option<BufferId> {
        self.client
            .state()
            .nodes
            .get(&node_id)
            .and_then(|node| node.buffer_view.as_ref())
            .map(|buffer_view| buffer_view.buffer_id)
    }

    async fn maybe_send_focus_sequence(
        &mut self,
        buffer_id: BufferId,
        focused: bool,
    ) -> Result<bool> {
        if !self
            .client
            .state()
            .snapshots
            .get(&buffer_id)
            .is_some_and(|snapshot| snapshot.focus_reporting)
        {
            return Ok(false);
        }
        let bytes = if focused {
            b"\x1b[I".to_vec()
        } else {
            b"\x1b[O".to_vec()
        };
        self.send_input_only(buffer_id, bytes).await?;
        Ok(true)
    }

    async fn send_input_only(&self, buffer_id: BufferId, bytes: Vec<u8>) -> Result<()> {
        self.client
            .request_message(ClientMessage::Input(InputRequest::Send {
                request_id: self.client.next_request_id(),
                buffer_id,
                bytes,
            }))
            .await?;
        Ok(())
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

fn action_is_local_terminal_action(action: &Action) -> bool {
    match action {
        Action::Chain(actions) => actions.iter().all(action_is_local_terminal_action),
        Action::EnterMode { mode } | Action::ToggleMode { mode } => {
            mode == SEARCH_MODE || mode == SELECT_MODE
        }
        Action::ScrollLineUp
        | Action::ScrollLineDown
        | Action::ScrollPageUp
        | Action::ScrollPageDown
        | Action::ScrollToTop
        | Action::ScrollToBottom
        | Action::FollowOutput
        | Action::EnterSearchMode
        | Action::SearchNext
        | Action::SearchPrev
        | Action::CommitSearch
        | Action::CancelSearch
        | Action::EnterSelect { .. }
        | Action::SelectMove { .. }
        | Action::CopySelection
        | Action::CancelSelection => true,
        _ => false,
    }
}

fn action_requires_local_context(action: &Action) -> bool {
    match action {
        Action::Chain(actions) => actions.iter().all(action_requires_local_context),
        Action::EnterSearchMode
        | Action::SearchNext
        | Action::SearchPrev
        | Action::EnterSelect { .. } => true,
        Action::EnterMode { mode } | Action::ToggleMode { mode } => {
            mode == SEARCH_MODE || mode == SELECT_MODE
        }
        _ => false,
    }
}

fn compute_search_matches(lines: &[String], query: &str) -> Vec<SearchMatch> {
    if query.is_empty() {
        return Vec::new();
    }

    let mut matches = Vec::new();
    for (line_index, line) in lines.iter().enumerate() {
        let mut search_from = 0;
        while let Some(found) = line[search_from..].find(query) {
            let byte_start = search_from + found;
            let byte_end = byte_start + query.len();
            let start_column = display_width(&line[..byte_start]);
            let end_column =
                start_column.saturating_add(display_width(&line[byte_start..byte_end]));
            matches.push(SearchMatch {
                line: u64::try_from(line_index).unwrap_or(u64::MAX),
                start_column,
                end_column,
            });
            search_from = byte_end.max(search_from + 1);
        }
    }
    matches
}

#[doc(hidden)]
pub fn benchmark_search_matches(lines: &[String], query: &str) -> Vec<SearchMatch> {
    compute_search_matches(lines, query)
}

#[doc(hidden)]
pub fn benchmark_serialize_selection(lines: &[String], selection: &SelectionState) -> String {
    serialize_selection(lines, selection)
}

fn serialize_selection(lines: &[String], selection: &SelectionState) -> String {
    match selection.kind {
        SelectionKind::Line => serialize_line_selection(lines, selection),
        SelectionKind::Block => serialize_block_selection(lines, selection),
        SelectionKind::Character => serialize_character_selection(lines, selection),
    }
}

fn serialize_character_selection(lines: &[String], selection: &SelectionState) -> String {
    let (start, end) = ordered_points(selection.anchor, selection.cursor);
    let mut parts = Vec::new();
    for line_index in start.line..=end.line {
        let line = lines
            .get(usize::try_from(line_index).unwrap_or(usize::MAX))
            .map(String::as_str)
            .unwrap_or("");
        let fragment = if start.line == end.line {
            slice_display_range(line, start.column, end.column.saturating_add(1))
        } else if line_index == start.line {
            slice_display_range(line, start.column, u16::MAX)
        } else if line_index == end.line {
            slice_display_range(line, 0, end.column.saturating_add(1))
        } else {
            line.to_owned()
        };
        parts.push(fragment);
    }
    parts.join("\n")
}

fn serialize_line_selection(lines: &[String], selection: &SelectionState) -> String {
    let start_line = selection.anchor.line.min(selection.cursor.line);
    let end_line = selection.anchor.line.max(selection.cursor.line);
    (start_line..=end_line)
        .map(|line_index| {
            lines
                .get(usize::try_from(line_index).unwrap_or(usize::MAX))
                .cloned()
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn serialize_block_selection(lines: &[String], selection: &SelectionState) -> String {
    let start_line = selection.anchor.line.min(selection.cursor.line);
    let end_line = selection.anchor.line.max(selection.cursor.line);
    let start_column = selection.anchor.column.min(selection.cursor.column);
    let end_column = selection
        .anchor
        .column
        .max(selection.cursor.column)
        .saturating_add(1);
    (start_line..=end_line)
        .map(|line_index| {
            let line = lines
                .get(usize::try_from(line_index).unwrap_or(usize::MAX))
                .map(String::as_str)
                .unwrap_or("");
            slice_display_range(line, start_column, end_column)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn ordered_points(left: SelectionPoint, right: SelectionPoint) -> (SelectionPoint, SelectionPoint) {
    if (left.line, left.column) <= (right.line, right.column) {
        (left, right)
    } else {
        (right, left)
    }
}

fn slice_display_range(line: &str, start_column: u16, end_column: u16) -> String {
    if start_column >= end_column {
        return String::new();
    }

    let mut column = 0_u16;
    let mut output = String::new();
    for grapheme in UnicodeSegmentation::graphemes(line, true) {
        let width = display_width(grapheme).max(1);
        let next_column = column.saturating_add(width);
        if next_column > start_column && column < end_column {
            output.push_str(grapheme);
        }
        if column >= end_column {
            break;
        }
        column = next_column;
    }
    output
}

fn encode_mouse_event(leaf: &LeafFrame, event: MouseEvent) -> Result<Vec<u8>> {
    let origin_x = clamp_origin(leaf.rect.origin.x);
    let origin_y = clamp_origin(leaf.rect.origin.y).saturating_add(1);
    let local_column = event
        .column
        .checked_sub(origin_x)
        .ok_or_else(|| MuxError::invalid_input("mouse event fell outside pane bounds"))?;
    let local_row = event
        .row
        .checked_sub(origin_y)
        .ok_or_else(|| MuxError::invalid_input("mouse event fell outside pane content"))?;

    let mut code = 0_u16;
    if event.modifiers.shift {
        code |= 0b00100;
    }
    if event.modifiers.alt {
        code |= 0b01000;
    }
    if event.modifiers.ctrl {
        code |= 0b10000;
    }

    let suffix = match event.kind {
        MouseEventKind::Press(button) => {
            code |= mouse_button_code(button);
            'M'
        }
        MouseEventKind::Release(button) => {
            code |= button.map(mouse_button_code).unwrap_or(0b11);
            'm'
        }
        MouseEventKind::Drag(button) => {
            code |= 0b100000 | mouse_button_code(button);
            'M'
        }
        MouseEventKind::WheelUp => {
            code |= 0b1_000000;
            'M'
        }
        MouseEventKind::WheelDown => {
            code |= 0b1_000001;
            'M'
        }
    };

    Ok(format!(
        "\x1b[<{code};{};{}{suffix}",
        local_column.saturating_add(1),
        local_row.saturating_add(1)
    )
    .into_bytes())
}

fn mouse_button_code(button: MouseButton) -> u16 {
    match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    }
}

fn clamp_origin(value: i32) -> u16 {
    u16::try_from(value.max(0)).unwrap_or(u16::MAX)
}

fn display_width(text: &str) -> u16 {
    u16::try_from(UnicodeWidthStr::width(text)).unwrap_or(u16::MAX)
}

fn prepend_actions(pending: &mut VecDeque<Action>, actions: Vec<Action>) {
    for action in actions.into_iter().rev() {
        pending.push_front(action);
    }
}

fn prepend_actions_with_limit(
    pending: &mut VecDeque<Action>,
    actions: Vec<Action>,
    expansions: &mut usize,
) -> Result<()> {
    *expansions = expansions.saturating_add(actions.len());
    if *expansions > MAX_EXPANDED_ACTIONS {
        return Err(MuxError::invalid_input("action expansion limit reached"));
    }
    prepend_actions(pending, actions);
    Ok(())
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
            let percent = u32::from(percent.clamp(1, 100));
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
        KeyEvent::Home => Ok(KeyToken::Home),
        KeyEvent::End => Ok(KeyToken::End),
        KeyEvent::Insert => Ok(KeyToken::Insert),
        KeyEvent::Delete => Ok(KeyToken::Delete),
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
            KeyToken::Home => bytes.extend_from_slice(b"\x1b[H"),
            KeyToken::End => bytes.extend_from_slice(b"\x1b[F"),
            KeyToken::Insert => bytes.extend_from_slice(b"\x1b[2~"),
            KeyToken::Delete => bytes.extend_from_slice(b"\x1b[3~"),
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

async fn rollback_created_buffer_on_error<T, U>(
    configured: &mut ConfiguredClient<U>,
    buffer_id: Option<BufferId>,
    operation: &str,
    result: Result<T>,
) -> Result<T>
where
    U: Transport,
{
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            if let Some(buffer_id) = buffer_id {
                configured
                    .rollback_created_buffer(buffer_id, operation)
                    .await;
            }
            Err(error)
        }
    }
}

fn event_name(event: &ServerEvent) -> &'static str {
    match event {
        ServerEvent::SessionCreated(_) => "session_created",
        ServerEvent::SessionClosed(_) => "session_closed",
        ServerEvent::SessionRenamed(_) => "session_renamed",
        ServerEvent::BufferCreated(_) => "buffer_created",
        ServerEvent::BufferDetached(_) => "buffer_detached",
        ServerEvent::NodeChanged(_) => "node_changed",
        ServerEvent::FloatingChanged(_) => "floating_changed",
        ServerEvent::FocusChanged(_) => "focus_changed",
        ServerEvent::RenderInvalidated(_) => "render_invalidated",
        ServerEvent::ClientChanged(_) => "client_changed",
    }
}

fn event_info(
    name: &str,
    event: &ServerEvent,
    fallback_session_id: Option<SessionId>,
) -> EventInfo {
    let mut info = base_event_info(name);
    match event {
        ServerEvent::SessionCreated(event) => info.session_id = Some(event.session.id),
        ServerEvent::SessionClosed(event) => info.session_id = Some(event.session_id),
        ServerEvent::SessionRenamed(event) => info.session_id = Some(event.session_id),
        ServerEvent::BufferCreated(event) => {
            info.buffer_id = Some(event.buffer.id);
            info.node_id = event.buffer.attachment_node_id;
        }
        ServerEvent::BufferDetached(event) => info.buffer_id = Some(event.buffer_id),
        ServerEvent::NodeChanged(event) => info.session_id = Some(event.session_id),
        ServerEvent::FloatingChanged(event) => {
            info.session_id = Some(event.session_id);
            info.floating_id = event.floating_id;
        }
        ServerEvent::FocusChanged(event) => {
            info.session_id = Some(event.session_id);
            info.node_id = event.focused_leaf_id;
            info.floating_id = event.focused_floating_id;
        }
        ServerEvent::RenderInvalidated(event) => info.buffer_id = Some(event.buffer_id),
        ServerEvent::ClientChanged(event) => {
            info.session_id = event.client.current_session_id;
            info.previous_session_id = event.previous_session_id;
            info.client_id = Some(event.client.id);
        }
    }
    if info.session_id.is_none() {
        info.session_id = fallback_session_id;
    }
    info
}

fn base_event_info(name: &str) -> EventInfo {
    EventInfo {
        name: name.to_owned(),
        session_id: None,
        previous_session_id: None,
        client_id: None,
        buffer_id: None,
        node_id: None,
        floating_id: None,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use embers_core::{ActivityState, BufferId, FloatingId, NodeId, PtySize, RequestId, SessionId};
    use embers_protocol::{
        BufferLocation, BufferLocationAttachment, BufferRecord, BufferRecordKind,
        BufferRecordState, BufferWithLocationResponse, FloatingChangedEvent, ServerEvent,
        ServerResponse,
    };
    use tempfile::tempdir;

    use super::{ConfiguredClient, SearchPrompt, event_info};
    use crate::client::MuxClient;
    use crate::config::{ConfigDiscoveryOptions, ConfigManager};
    use crate::input::NORMAL_MODE;
    use crate::testing::FakeTransport;

    #[test]
    fn attached_buffer_location_accepts_session_and_floating_locations() {
        assert_eq!(
            ConfiguredClient::<crate::testing::FakeTransport>::attached_buffer_location(
                Some(BufferId(7)),
                BufferLocation::session(BufferId(7), SessionId(2), NodeId(5)),
                "buffer focus",
            )
            .expect("session attachment should validate"),
            (SessionId(2), NodeId(5))
        );
        assert_eq!(
            ConfiguredClient::<crate::testing::FakeTransport>::attached_buffer_location(
                Some(BufferId(7)),
                BufferLocation::floating(BufferId(7), SessionId(2), NodeId(5), FloatingId(9)),
                "buffer reveal",
            )
            .expect("floating attachment should validate"),
            (SessionId(2), NodeId(5))
        );
    }

    #[test]
    fn attached_buffer_location_accepts_history_helper_locations_without_source_id_match() {
        assert_eq!(
            ConfiguredClient::<crate::testing::FakeTransport>::attached_buffer_location(
                None,
                BufferLocation::session(BufferId(8), SessionId(2), NodeId(5)),
                "buffer history",
            )
            .expect("history helper attachment should validate"),
            (SessionId(2), NodeId(5))
        );
    }

    #[test]
    fn attached_buffer_location_rejects_mismatched_or_detached_locations() {
        let mismatch = ConfiguredClient::<crate::testing::FakeTransport>::attached_buffer_location(
            Some(BufferId(7)),
            BufferLocation::session(BufferId(8), SessionId(2), NodeId(5)),
            "buffer focus",
        )
        .expect_err("mismatched buffer ids should fail");
        let detached = ConfiguredClient::<crate::testing::FakeTransport>::attached_buffer_location(
            Some(BufferId(7)),
            BufferLocation {
                buffer_id: BufferId(7),
                attachment: BufferLocationAttachment::Detached,
            },
            "buffer reveal",
        )
        .expect_err("detached locations should fail");

        assert!(
            mismatch
                .to_string()
                .contains("returned location for buffer 8")
        );
        assert!(detached.to_string().contains("detached location"));
    }

    #[test]
    fn buffer_location_from_response_accepts_buffer_with_location() {
        let location = BufferLocation::session(BufferId(8), SessionId(2), NodeId(5));
        let response = ServerResponse::BufferWithLocation(
            BufferWithLocationResponse::new(
                RequestId(1),
                BufferRecord {
                    id: BufferId(8),
                    title: "helper".to_owned(),
                    command: Vec::new(),
                    cwd: None,
                    kind: BufferRecordKind::Helper,
                    state: BufferRecordState::Created,
                    pid: None,
                    attachment_node_id: Some(NodeId(5)),
                    read_only: true,
                    helper_source_buffer_id: Some(BufferId(7)),
                    helper_scope: Some(embers_protocol::BufferHistoryScope::Visible),
                    pty_size: PtySize::new(80, 24),
                    activity: ActivityState::Idle,
                    last_snapshot_seq: 0,
                    exit_code: None,
                    env: Default::default(),
                },
                location,
                false,
            )
            .expect("buffer and location ids should match"),
        );

        assert_eq!(
            ConfiguredClient::<crate::testing::FakeTransport>::buffer_location_from_response(
                response,
                "buffer history",
            )
            .expect("buffer-with-location response should validate"),
            location
        );
    }

    #[test]
    fn floating_changed_event_info_includes_floating_id() {
        let info = event_info(
            "floating_changed",
            &ServerEvent::FloatingChanged(FloatingChangedEvent {
                session_id: SessionId(2),
                floating_id: Some(FloatingId(9)),
            }),
            None,
        );

        assert_eq!(info.session_id, Some(SessionId(2)));
        assert_eq!(info.floating_id, Some(FloatingId(9)));
    }

    #[test]
    fn finish_config_reload_clears_search_prompt_when_mode_falls_back_to_normal() {
        let tempdir = tempdir().expect("create tempdir");
        let config_path = tempdir.path().join("config.rhai");
        fs::write(&config_path, "define_mode(\"custom\");").expect("write custom config");
        let config = ConfigManager::load(
            ConfigDiscoveryOptions::default().with_project_config_dir(tempdir.path()),
        )
        .expect("load config");
        let client = MuxClient::new(FakeTransport::default());
        let mut configured = ConfiguredClient::new(client, config);
        configured.input_state.set_mode("custom".to_owned());
        configured.search_prompt = Some(SearchPrompt {
            node_id: NodeId(7),
            query: "stale".to_owned(),
        });

        fs::write(&config_path, "").expect("rewrite config without custom mode");

        configured.reload_config().expect("reload config");

        assert_eq!(configured.input_state.current_mode(), NORMAL_MODE);
        assert_eq!(configured.search_prompt, None);
    }
}
