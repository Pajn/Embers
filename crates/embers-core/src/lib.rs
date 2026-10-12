pub mod clipboard;
pub mod diagnostics;
pub mod error;
pub mod geometry;
pub mod ids;
pub mod metadata;
pub mod passthrough;
pub mod snapshot;

pub use clipboard::{ClipboardSelection, ClipboardWrite, MAX_CLIPBOARD_WRITE_BYTES};
pub use diagnostics::{
    RequestContext, format_focus_path, format_tree_dump, init_server_tracing, init_test_tracing,
    init_tracing, new_request_id, request_span,
};
pub use error::{ErrorCode, MuxError, Result, WireError};
pub use geometry::{CellSize, FloatGeometry, Point, PtySize, Rect, Size, SplitDirection};
pub use ids::{BufferId, ClientId, FloatingId, IdAllocator, NodeId, RequestId, SessionId};
pub use metadata::{ActivityState, EntityMetadata, Timestamp};
pub use passthrough::{MAX_PASSTHROUGH_BYTES, PassthroughWrite};
pub use snapshot::{
    CellAttrs, CursorPosition, CursorShape, CursorState, Hyperlink, SnapshotLine, StyledRun,
    TermColor, TerminalModes, TerminalSnapshot,
};
