pub mod model;
pub mod state;

mod buffer_runtime;
mod config;
mod persist;
mod protocol;
mod server;
mod terminal_backend;

pub use buffer_runtime::{
    BufferRuntimeCallbacks, BufferRuntimeHandle, BufferRuntimePipeStatus,
    BufferRuntimePipeStopReason, BufferRuntimeStatus, BufferRuntimeUpdate, RuntimeKeeperCli,
    run_runtime_keeper,
};
pub use config::{ResourceLimits, SOCKET_ENV_VAR, ServerConfig};
pub use model::{
    Buffer, BufferAttachment, BufferPipe, BufferPipeState, BufferPipeStopReason, BufferState,
    BufferViewNode, BufferViewState, ExitedBuffer, FloatingWindow, InterruptedBuffer, Node,
    RunningBuffer, Session, SplitNode, TabEntry, TabsNode,
};
pub use server::{Server, ServerHandle};
pub use state::ServerState;
pub use terminal_backend::{
    AlacrittyTerminalBackend, BackendDamage, BackendMetadata, BackendScrollbackSlice,
    RawByteRouter, TerminalBackend,
};
