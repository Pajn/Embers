use embers_core::{
    ActivityState, BufferId, FloatGeometry, FloatingId, NodeId, PtySize, RequestId, SessionId,
    SplitDirection, WireError,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PingRequest {
    pub request_id: RequestId,
    pub payload: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PingResponse {
    pub request_id: RequestId,
    pub payload: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionRequest {
    Create {
        request_id: RequestId,
        name: String,
    },
    List {
        request_id: RequestId,
    },
    Get {
        request_id: RequestId,
        session_id: SessionId,
    },
    Close {
        request_id: RequestId,
        session_id: SessionId,
        force: bool,
    },
    AddRootTab {
        request_id: RequestId,
        session_id: SessionId,
        title: String,
        buffer_id: Option<BufferId>,
        child_node_id: Option<NodeId>,
    },
    SelectRootTab {
        request_id: RequestId,
        session_id: SessionId,
        index: usize,
    },
    RenameRootTab {
        request_id: RequestId,
        session_id: SessionId,
        index: usize,
        title: String,
    },
    CloseRootTab {
        request_id: RequestId,
        session_id: SessionId,
        index: usize,
    },
}

impl SessionRequest {
    pub fn request_id(&self) -> RequestId {
        match self {
            Self::Create { request_id, .. }
            | Self::List { request_id }
            | Self::Get { request_id, .. }
            | Self::Close { request_id, .. }
            | Self::AddRootTab { request_id, .. }
            | Self::SelectRootTab { request_id, .. }
            | Self::RenameRootTab { request_id, .. }
            | Self::CloseRootTab { request_id, .. } => *request_id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BufferRequest {
    Create {
        request_id: RequestId,
        title: Option<String>,
        command: Vec<String>,
        cwd: Option<String>,
    },
    List {
        request_id: RequestId,
        session_id: Option<SessionId>,
        attached_only: bool,
        detached_only: bool,
    },
    Get {
        request_id: RequestId,
        buffer_id: BufferId,
    },
    Detach {
        request_id: RequestId,
        buffer_id: BufferId,
    },
    Kill {
        request_id: RequestId,
        buffer_id: BufferId,
        force: bool,
    },
    Capture {
        request_id: RequestId,
        buffer_id: BufferId,
    },
}

impl BufferRequest {
    pub fn request_id(&self) -> RequestId {
        match self {
            Self::Create { request_id, .. }
            | Self::List { request_id, .. }
            | Self::Get { request_id, .. }
            | Self::Detach { request_id, .. }
            | Self::Kill { request_id, .. }
            | Self::Capture { request_id, .. } => *request_id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeRequest {
    GetTree {
        request_id: RequestId,
        session_id: SessionId,
    },
    Split {
        request_id: RequestId,
        leaf_node_id: NodeId,
        direction: SplitDirection,
        new_buffer_id: BufferId,
    },
    WrapInTabs {
        request_id: RequestId,
        node_id: NodeId,
        title: String,
    },
    AddTab {
        request_id: RequestId,
        tabs_node_id: NodeId,
        title: String,
        buffer_id: Option<BufferId>,
        child_node_id: Option<NodeId>,
    },
    SelectTab {
        request_id: RequestId,
        tabs_node_id: NodeId,
        index: usize,
    },
    Focus {
        request_id: RequestId,
        session_id: SessionId,
        node_id: NodeId,
    },
    Close {
        request_id: RequestId,
        node_id: NodeId,
    },
    MoveBufferToNode {
        request_id: RequestId,
        buffer_id: BufferId,
        target_leaf_node_id: NodeId,
    },
    Resize {
        request_id: RequestId,
        node_id: NodeId,
        sizes: Vec<u16>,
    },
}

impl NodeRequest {
    pub fn request_id(&self) -> RequestId {
        match self {
            Self::GetTree { request_id, .. }
            | Self::Split { request_id, .. }
            | Self::WrapInTabs { request_id, .. }
            | Self::AddTab { request_id, .. }
            | Self::SelectTab { request_id, .. }
            | Self::Focus { request_id, .. }
            | Self::Close { request_id, .. }
            | Self::MoveBufferToNode { request_id, .. }
            | Self::Resize { request_id, .. } => *request_id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FloatingRequest {
    Create {
        request_id: RequestId,
        session_id: SessionId,
        root_node_id: Option<NodeId>,
        buffer_id: Option<BufferId>,
        geometry: FloatGeometry,
        title: Option<String>,
    },
    Close {
        request_id: RequestId,
        floating_id: FloatingId,
    },
    Move {
        request_id: RequestId,
        floating_id: FloatingId,
        geometry: FloatGeometry,
    },
    Focus {
        request_id: RequestId,
        floating_id: FloatingId,
    },
}

impl FloatingRequest {
    pub fn request_id(&self) -> RequestId {
        match self {
            Self::Create { request_id, .. }
            | Self::Close { request_id, .. }
            | Self::Move { request_id, .. }
            | Self::Focus { request_id, .. } => *request_id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputRequest {
    Send {
        request_id: RequestId,
        buffer_id: BufferId,
        bytes: Vec<u8>,
    },
    Resize {
        request_id: RequestId,
        buffer_id: BufferId,
        cols: u16,
        rows: u16,
    },
}

impl InputRequest {
    pub fn request_id(&self) -> RequestId {
        match self {
            Self::Send { request_id, .. } | Self::Resize { request_id, .. } => *request_id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubscribeRequest {
    pub request_id: RequestId,
    pub session_id: Option<SessionId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnsubscribeRequest {
    pub request_id: RequestId,
    pub subscription_id: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientMessage {
    Ping(PingRequest),
    Session(SessionRequest),
    Buffer(BufferRequest),
    Node(NodeRequest),
    Floating(FloatingRequest),
    Input(InputRequest),
    Subscribe(SubscribeRequest),
    Unsubscribe(UnsubscribeRequest),
}

impl ClientMessage {
    pub fn request_id(&self) -> RequestId {
        match self {
            Self::Ping(request) => request.request_id,
            Self::Session(request) => request.request_id(),
            Self::Buffer(request) => request.request_id(),
            Self::Node(request) => request.request_id(),
            Self::Floating(request) => request.request_id(),
            Self::Input(request) => request.request_id(),
            Self::Subscribe(request) => request.request_id,
            Self::Unsubscribe(request) => request.request_id,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferRecordState {
    Created,
    Running,
    Exited,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRecord {
    pub id: SessionId,
    pub name: String,
    pub root_node_id: NodeId,
    pub floating_ids: Vec<FloatingId>,
    pub focused_leaf_id: Option<NodeId>,
    pub focused_floating_id: Option<FloatingId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferRecord {
    pub id: BufferId,
    pub title: String,
    pub command: Vec<String>,
    pub cwd: Option<String>,
    pub state: BufferRecordState,
    pub attachment_node_id: Option<NodeId>,
    pub pty_size: PtySize,
    pub activity: ActivityState,
    pub last_snapshot_seq: u64,
    pub exit_code: Option<i32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeRecordKind {
    BufferView,
    Split,
    Tabs,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferViewRecord {
    pub buffer_id: BufferId,
    pub focused: bool,
    pub zoomed: bool,
    pub follow_output: bool,
    pub last_render_size: PtySize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitRecord {
    pub direction: SplitDirection,
    pub child_ids: Vec<NodeId>,
    pub sizes: Vec<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabRecord {
    pub title: String,
    pub child_id: NodeId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabsRecord {
    pub active: usize,
    pub tabs: Vec<TabRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeRecord {
    pub id: NodeId,
    pub session_id: SessionId,
    pub parent_id: Option<NodeId>,
    pub kind: NodeRecordKind,
    pub buffer_view: Option<BufferViewRecord>,
    pub split: Option<SplitRecord>,
    pub tabs: Option<TabsRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FloatingRecord {
    pub id: FloatingId,
    pub session_id: SessionId,
    pub root_node_id: NodeId,
    pub title: Option<String>,
    pub geometry: FloatGeometry,
    pub focused: bool,
    pub visible: bool,
    pub close_on_empty: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSnapshot {
    pub session: SessionRecord,
    pub nodes: Vec<NodeRecord>,
    pub buffers: Vec<BufferRecord>,
    pub floating: Vec<FloatingRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OkResponse {
    pub request_id: RequestId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErrorResponse {
    pub request_id: Option<RequestId>,
    pub error: WireError,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionsResponse {
    pub request_id: RequestId,
    pub sessions: Vec<SessionRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSnapshotResponse {
    pub request_id: RequestId,
    pub snapshot: SessionSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuffersResponse {
    pub request_id: RequestId,
    pub buffers: Vec<BufferRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferResponse {
    pub request_id: RequestId,
    pub buffer: BufferRecord,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FloatingListResponse {
    pub request_id: RequestId,
    pub floating: Vec<FloatingRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FloatingResponse {
    pub request_id: RequestId,
    pub floating: FloatingRecord,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubscriptionAckResponse {
    pub request_id: RequestId,
    pub subscription_id: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotResponse {
    pub request_id: RequestId,
    pub buffer_id: BufferId,
    pub sequence: u64,
    pub size: PtySize,
    pub lines: Vec<String>,
    pub title: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerResponse {
    Pong(PingResponse),
    Ok(OkResponse),
    Error(ErrorResponse),
    Sessions(SessionsResponse),
    SessionSnapshot(SessionSnapshotResponse),
    Buffers(BuffersResponse),
    Buffer(BufferResponse),
    FloatingList(FloatingListResponse),
    Floating(FloatingResponse),
    SubscriptionAck(SubscriptionAckResponse),
    Snapshot(SnapshotResponse),
}

impl ServerResponse {
    pub fn request_id(&self) -> Option<RequestId> {
        match self {
            Self::Pong(response) => Some(response.request_id),
            Self::Ok(response) => Some(response.request_id),
            Self::Error(response) => response.request_id,
            Self::Sessions(response) => Some(response.request_id),
            Self::SessionSnapshot(response) => Some(response.request_id),
            Self::Buffers(response) => Some(response.request_id),
            Self::Buffer(response) => Some(response.request_id),
            Self::FloatingList(response) => Some(response.request_id),
            Self::Floating(response) => Some(response.request_id),
            Self::SubscriptionAck(response) => Some(response.request_id),
            Self::Snapshot(response) => Some(response.request_id),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionCreatedEvent {
    pub session: SessionRecord,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionClosedEvent {
    pub session_id: SessionId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferCreatedEvent {
    pub buffer: BufferRecord,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferDetachedEvent {
    pub buffer_id: BufferId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeChangedEvent {
    pub session_id: SessionId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FloatingChangedEvent {
    pub session_id: SessionId,
    pub floating_id: Option<FloatingId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FocusChangedEvent {
    pub session_id: SessionId,
    pub focused_leaf_id: Option<NodeId>,
    pub focused_floating_id: Option<FloatingId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderInvalidatedEvent {
    pub buffer_id: BufferId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerEvent {
    SessionCreated(SessionCreatedEvent),
    SessionClosed(SessionClosedEvent),
    BufferCreated(BufferCreatedEvent),
    BufferDetached(BufferDetachedEvent),
    NodeChanged(NodeChangedEvent),
    FloatingChanged(FloatingChangedEvent),
    FocusChanged(FocusChangedEvent),
    RenderInvalidated(RenderInvalidatedEvent),
}

impl ServerEvent {
    pub fn session_id(&self) -> Option<SessionId> {
        match self {
            Self::SessionCreated(event) => Some(event.session.id),
            Self::SessionClosed(event) => Some(event.session_id),
            Self::BufferCreated(_) => None,
            Self::BufferDetached(_) => None,
            Self::NodeChanged(event) => Some(event.session_id),
            Self::FloatingChanged(event) => Some(event.session_id),
            Self::FocusChanged(event) => Some(event.session_id),
            Self::RenderInvalidated(_) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerEnvelope {
    Response(ServerResponse),
    Event(ServerEvent),
}
