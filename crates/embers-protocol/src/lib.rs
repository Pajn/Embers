pub mod client;
pub mod codec;
pub mod framing;
pub mod types;

/// Code generated from `schema/embers.fbs`. Regenerate it with `cargo gen-protocol` rather than
/// editing it by hand; `include!` keeps it out of rustfmt's reach.
pub mod generated {
    #![allow(
        clippy::all,
        dead_code,
        non_camel_case_types,
        non_snake_case,
        non_upper_case_globals,
        unused_imports
    )]
    include!("generated/embers_generated.rs");
}

/// The version of the protocol this build speaks. Peers exchange it in
/// [`HelloRequest`] and refuse one that speaks another. Bump it for a change a
/// peer built before it would misread; a message or field added in a way older
/// peers skip or answer with an error does not need a bump.
pub const PROTOCOL_VERSION: u32 = 1;

/// The release this build is from, sent alongside [`PROTOCOL_VERSION`] so a
/// refusal can name both releases.
pub const SOFTWARE_VERSION: &str = env!("CARGO_PKG_VERSION");

pub use client::ProtocolClient;
pub use codec::{
    ProtocolError, decode_client_message, decode_server_envelope, encode_client_message,
    encode_server_envelope,
};
pub use framing::{
    FrameType, MAX_FRAME_LEN, RawFrame, read_frame, write_frame, write_frame_no_flush,
};
pub use types::{
    BufferCreatedEvent, BufferDetachedEvent, BufferHistoryPlacement, BufferHistoryScope,
    BufferLocation, BufferLocationAttachment, BufferLocationResponse, BufferPipeChangedEvent,
    BufferPipeRecord, BufferPipeState, BufferPipeStopReason, BufferRecord, BufferRecordKind,
    BufferRecordState, BufferRequest, BufferResponse, BufferViewRecord, BufferWithLocationResponse,
    BuffersResponse, ClientChangedEvent, ClientMessage, ClientRecord, ClientRequest,
    ClientResponse, ClientsResponse, ClipboardEvent, DisplayMessageEvent, EnvironmentResponse,
    ErrorResponse, FloatingChangedEvent, FloatingListResponse, FloatingRecord, FloatingRequest,
    FloatingResponse, FocusChangedEvent, HelloRequest, HelloResponse, InputRequest,
    KillServerRequest, NodeBreakDestination, NodeChangedEvent, NodeJoinPlacement, NodeRecord,
    NodeRecordKind, NodeRequest, OkResponse, PassthroughEvent, PingRequest, PingResponse,
    RenderInvalidatedEvent, ScrollbackSliceResponse, ServerEnvelope, ServerEvent, ServerResponse,
    SessionClosedEvent, SessionCreatedEvent, SessionRecord, SessionRenamedEvent, SessionRequest,
    SessionSnapshot, SessionSnapshotResponse, SessionsResponse, SnapshotResponse, SplitRecord,
    SubscribeRequest, SubscriptionAckResponse, SynchronizePanesMode, TabRecord, TabsRecord,
    UnsubscribeRequest, VisibleSnapshotResponse, buffer_ids_in_subtree, program_name,
};
