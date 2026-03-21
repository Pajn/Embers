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
pub use framing::{MAX_FRAME_LEN, read_frame, write_frame};
pub use types::{
    ClientMessage, ErrorResponse, HeartbeatEvent, PingRequest, PingResponse, ServerEnvelope,
    ServerEvent, ServerResponse,
};
