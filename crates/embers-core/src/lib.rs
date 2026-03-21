pub mod diagnostics;
pub mod error;
pub mod geometry;
pub mod ids;
pub mod metadata;
pub mod snapshot;

pub use diagnostics::{
    RequestContext, format_focus_path, format_tree_dump, init_test_tracing, init_tracing,
    new_request_id, request_span,
};
pub use error::{ErrorCode, MuxError, Result, WireError};
pub use geometry::{FloatGeometry, Point, PtySize, Rect, Size, SplitDirection};
pub use ids::{BufferId, ClientId, FloatingId, IdAllocator, NodeId, RequestId, SessionId};
pub use metadata::{ActivityState, EntityMetadata, Timestamp};
pub use snapshot::{
    CursorPosition, CursorShape, CursorState, SnapshotLine, TerminalModes, TerminalSnapshot,
};
