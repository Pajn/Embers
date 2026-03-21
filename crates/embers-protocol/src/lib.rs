pub mod client;
pub mod codec;
pub mod framing;
pub mod types;

pub mod generated {
    #![allow(
        clippy::all,
        dead_code,
        non_camel_case_types,
        non_snake_case,
        non_upper_case_globals,
        unused_imports
    )]
    include!(concat!(env!("OUT_DIR"), "/embers_generated.rs"));
}

pub use client::ProtocolClient;
pub use codec::{
    ProtocolError, decode_client_message, decode_server_envelope, encode_client_message,
    encode_server_envelope,
};
pub use framing::{
    FrameType, MAX_FRAME_LEN, RawFrame, read_frame, write_frame, write_frame_no_flush,
};
pub use types::{
    BufferCreatedEvent, BufferDetachedEvent, BufferRecord, BufferRecordState, BufferRequest,
    BufferResponse, BufferViewRecord, BuffersResponse, ClientMessage, ErrorResponse,
    FloatingChangedEvent, FloatingListResponse, FloatingRecord, FloatingRequest, FloatingResponse,
    FocusChangedEvent, InputRequest, NodeChangedEvent, NodeRecord, NodeRecordKind, NodeRequest,
    OkResponse, PingRequest, PingResponse, RenderInvalidatedEvent, ServerEnvelope, ServerEvent,
    ScrollbackSliceResponse, ServerResponse, SessionClosedEvent, SessionCreatedEvent,
    SessionRecord, SessionRequest, SessionSnapshot, SessionSnapshotResponse, SessionsResponse,
    SnapshotResponse, SplitRecord, SubscribeRequest, SubscriptionAckResponse, TabRecord,
    TabsRecord, UnsubscribeRequest, VisibleSnapshotResponse,
};
