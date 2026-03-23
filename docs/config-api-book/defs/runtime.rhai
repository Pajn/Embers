module static;



/// Read a named color from the active runtime palette, if it exists.
///
/// ReturnType: `RgbColor | ()`
fn color(theme: ThemeRuntimeApi, name: string) -> ?;

/// Build a full bar specification from left, center, and right segments.
fn bar(_: UiApi, left: array, center: array, right: array) -> BarSpec;

/// Create a [`BarSegment`] from a [`UiApi`] receiver and text using default styling.
///
/// `segment(_: UiApi, text: String) -> BarSegment` produces plain text with default
/// [`StyleSpec`] values and no click target.
fn segment(_: UiApi, text: string) -> BarSegment;

/// Create a [`BarSegment`] from a [`UiApi`] receiver, text, and an `options: Map`.
///
/// `segment(_: UiApi, text: String, options: Map) -> BarSegment` supports `fg`, `bg`,
/// `bold`, `italic`, `underline`, `dim`, and `target` keys to override styling and attach an
/// optional interaction target. `dim` is a boolean that renders the text with reduced
/// intensity for a muted appearance.
fn segment(_: UiApi, text: string, options: map) -> BarSegment;

/// Read an environment variable, if it is set.
///
/// ReturnType: `string | ()`
fn env(_: SystemApi, name: string) -> ?;

/// Return the current Unix timestamp in seconds.
fn now(_: SystemApi) -> int;

/// Resolve an executable from `PATH`, if it is found.
///
/// ReturnType: `string | ()`
fn which(_: SystemApi, name: string) -> ?;

/// Return the currently focused buffer, if any.
///
/// ReturnType: `BufferRef | ()`
fn current_buffer(mux: MuxApi) -> ?;

/// Return the currently focused floating window, if any.
///
/// ReturnType: `FloatingRef | ()`
fn current_floating(mux: MuxApi) -> ?;

/// Return the currently focused node, if any.
///
/// ReturnType: `NodeRef | ()`
fn current_node(mux: MuxApi) -> ?;

/// Return the current session reference, if any.
///
/// ReturnType: `SessionRef | ()`
fn current_session(mux: MuxApi) -> ?;

/// Return detached buffers in the current model snapshot.
fn detached_buffers(mux: MuxApi) -> array;

/// Find a buffer by numeric id. Returns `()` when it does not exist.
///
/// ReturnType: `BufferRef | ()`
fn find_buffer(mux: MuxApi, buffer_id: int) -> ?;

/// Find a floating window by numeric id. Returns `()` when it does not exist.
///
/// ReturnType: `FloatingRef | ()`
fn find_floating(mux: MuxApi, floating_id: int) -> ?;

/// Find a node by numeric id. Returns `()` when it does not exist.
///
/// ReturnType: `NodeRef | ()`
fn find_node(mux: MuxApi, node_id: int) -> ?;

/// Return every visible session.
fn sessions(mux: MuxApi) -> array;

/// Return visible buffers in the current model snapshot.
fn visible_buffers(mux: MuxApi) -> array;

/// Attach an existing buffer by id.
fn buffer_attach(_: TreeApi, buffer_id: int) -> TreeSpec;

/// Build a tree reference to the currently focused buffer.
fn buffer_current(_: TreeApi) -> TreeSpec;

/// Build an empty buffer tree node.
fn buffer_empty(_: TreeApi) -> TreeSpec;

/// Spawn a new buffer from a command array.
///
/// Supported `options` keys are `title` (`string`), `cwd` (`string`), and `env`
/// (`map<string, string>`). Unknown keys are rejected.
///
/// # Example
///
/// ```rhai
/// tree.buffer_spawn(["/bin/zsh"], #{ title: "shell" })
/// ```
fn buffer_spawn(_: TreeApi, command: array) -> TreeSpec;

fn buffer_spawn(_: TreeApi, command: array, options: map) -> TreeSpec;

/// Build a tree reference to the currently focused buffer.
fn current_buffer(_: TreeApi) -> TreeSpec;

/// Build a tree reference to the currently focused node.
fn current_node(_: TreeApi) -> TreeSpec;

/// Build a split with an explicit direction string.
fn split(_: TreeApi, direction: string, children: array) -> TreeSpec;

/// Build a split with explicit sizes for each child.
fn split(_: TreeApi, direction: string, children: array, sizes: array) -> TreeSpec;

/// Build a horizontal split.
fn split_h(_: TreeApi, children: array) -> TreeSpec;

/// Build a vertical split.
fn split_v(_: TreeApi, children: array) -> TreeSpec;

/// Build a single tab specification.
fn tab(_: TreeApi, title: string, tree: TreeSpec) -> TabSpec;

/// Build a tabs container with the first tab active.
fn tabs(_: TreeApi, tabs: array) -> TreeSpec;

/// Build a tabs container with an explicit active tab.
fn tabs_with_active(_: TreeApi, tabs: array, active: int) -> TreeSpec;

/// Cancel the active search.
fn cancel_search(_: ActionApi) -> Action;

/// Cancel the current selection.
fn cancel_selection(_: ActionApi) -> Action;

/// Chain multiple actions into one composite action.
fn chain(_: ActionApi, actions: array) -> Action;

/// Clear any partially-entered key sequence.
fn clear_pending_keys(_: ActionApi) -> Action;

/// Close the currently focused floating window.
fn close_floating(_: ActionApi) -> Action;

/// Close a floating window by id.
fn close_floating_id(_: ActionApi, floating_id: int) -> Action;

/// Close a view by node id.
fn close_node(_: ActionApi, node_id: int) -> Action;

/// Close the currently focused view.
fn close_view(_: ActionApi) -> Action;

/// Copy the current selection into the clipboard.
fn copy_selection(_: ActionApi) -> Action;

/// Detach the currently focused buffer.
fn detach_buffer(_: ActionApi) -> Action;

/// Detach a buffer by id.
fn detach_buffer_id(_: ActionApi, buffer_id: int) -> Action;

/// Enter a specific input mode by name.
fn enter_mode(_: ActionApi, mode: string) -> Action;

/// Enter incremental search mode.
fn enter_search_mode(_: ActionApi) -> Action;

/// Enter block selection mode.
fn enter_select_block(_: ActionApi) -> Action;

/// Enter character selection mode.
fn enter_select_char(_: ActionApi) -> Action;

/// Enter line selection mode.
fn enter_select_line(_: ActionApi) -> Action;

/// Focus a specific buffer by id.
fn focus_buffer(_: ActionApi, buffer_id: int) -> Action;

/// Focus the view below the current node.
fn focus_down(_: ActionApi) -> Action;

/// Focus the view to the left of the current node.
///
/// # Example
///
/// ```rhai
/// action.focus_left()
/// ```
fn focus_left(_: ActionApi) -> Action;

/// Focus the view to the right of the current node.
fn focus_right(_: ActionApi) -> Action;

/// Focus the view above the current node.
fn focus_up(_: ActionApi) -> Action;

/// Re-enable following live output.
fn follow_output(_: ActionApi) -> Action;

/// Insert a tab after a specific tabs node.
fn insert_tab_after(_: ActionApi, tabs_node_id: int, title: string, tree: TreeSpec) -> Action;

/// Insert a tab after the current tab in the focused tabs node.
fn insert_tab_after_current(_: ActionApi, title: string, tree: TreeSpec) -> Action;

/// Insert a tab before a specific tabs node.
fn insert_tab_before(_: ActionApi, tabs_node_id: int, title: string, tree: TreeSpec) -> Action;

/// Insert a tab before the current tab.
fn insert_tab_before_current(_: ActionApi, title: string, tree: TreeSpec) -> Action;

/// Kill the currently focused buffer.
fn kill_buffer(_: ActionApi) -> Action;

/// Kill a buffer by id.
fn kill_buffer_id(_: ActionApi, buffer_id: int) -> Action;

/// Leave the active input mode.
fn leave_mode(_: ActionApi) -> Action;

/// Move a buffer into a new floating window.
///
/// # Options
///
/// - `x` (i16): horizontal offset from the anchor (default: 0)
/// - `y` (i16): vertical offset from the anchor (default: 0)
/// - `width` (FloatingSize): window width, as a percentage (e.g., 50%) or pixel value (default: 50%)
/// - `height` (FloatingSize): window height, as a percentage or pixel value (default: 50%)
/// - `anchor` (FloatingAnchor): anchor point for positioning, e.g., "top_left", "center" (default: center)
/// - `title` (Option\<String\>): window title (default: none)
/// - `focus` (bool): whether to focus the window after creation (default: true)
/// - `close_on_empty` (bool): whether to close the window when its buffer empties (default: true)
fn move_buffer_to_floating(_: ActionApi, buffer_id: int, options: map) -> Action;

/// Move a buffer into a specific node.
fn move_buffer_to_node(_: ActionApi, buffer_id: int, node_id: int) -> Action;

/// Select the next tab in the currently focused tabs node.
fn next_current_tabs(_: ActionApi) -> Action;

/// Select the next tab in a specific tabs node.
fn next_tab(_: ActionApi, tabs_node_id: int) -> Action;

/// Build a no-op action.
fn noop(_: ActionApi) -> Action;

/// Emit a client notification.
fn notify(_: ActionApi, level: string, message: string) -> Action;

/// Open a floating view around the provided tree.
fn open_floating(_: ActionApi, tree: TreeSpec, options: map) -> Action;

/// Select the previous tab in the currently focused tabs node.
fn prev_current_tabs(_: ActionApi) -> Action;

/// Select the previous tab in a specific tabs node.
fn prev_tab(_: ActionApi, tabs_node_id: int) -> Action;

/// Replace the focused node with a new tree.
fn replace_current_with(_: ActionApi, tree: TreeSpec) -> Action;

/// Replace a specific node by id with a new tree.
fn replace_node(_: ActionApi, node_id: int, tree: TreeSpec) -> Action;

/// Reveal a specific buffer by id.
fn reveal_buffer(_: ActionApi, buffer_id: int) -> Action;

/// Run another named action by name.
fn run_named_action(_: ActionApi, name: string) -> Action;

/// Scroll one line downward in local scrollback.
fn scroll_line_down(_: ActionApi) -> Action;

/// Scroll one line upward in local scrollback.
fn scroll_line_up(_: ActionApi) -> Action;

/// Scroll one page downward in local scrollback.
fn scroll_page_down(_: ActionApi) -> Action;

/// Scroll one page upward in local scrollback.
fn scroll_page_up(_: ActionApi) -> Action;

/// Scroll to the bottom of local scrollback.
fn scroll_to_bottom(_: ActionApi) -> Action;

/// Scroll to the top of local scrollback.
fn scroll_to_top(_: ActionApi) -> Action;

/// Jump to the next search match.
fn search_next(_: ActionApi) -> Action;

/// Jump to the previous search match.
fn search_prev(_: ActionApi) -> Action;

/// Select a tab by index in the currently focused tabs node.
fn select_current_tabs(_: ActionApi, index: int) -> Action;

/// Move the active selection down.
fn select_move_down(_: ActionApi) -> Action;

/// Move the active selection left.
fn select_move_left(_: ActionApi) -> Action;

/// Move the active selection right.
fn select_move_right(_: ActionApi) -> Action;

/// Move the active selection up.
fn select_move_up(_: ActionApi) -> Action;

/// Select a tab by index in a specific tabs node.
fn select_tab(_: ActionApi, tabs_node_id: int, index: int) -> Action;

/// Send a string of bytes to a specific buffer.
fn send_bytes(_: ActionApi, buffer_id: int, bytes: string) -> Action;

/// Send raw byte values to a specific buffer.
fn send_bytes(_: ActionApi, buffer_id: int, bytes: array) -> Action;

/// Send a string of bytes to the focused buffer.
fn send_bytes_current(_: ActionApi, bytes: string) -> Action;

/// Send raw byte values to the focused buffer.
///
/// Use this when you need to emit an exact byte sequence instead of key notation.
///
/// # Example
///
/// ```rhai
/// // Send the ANSI "cursor up" sequence: ESC [ A
/// action.send_bytes_current([0x1b, 0x5b, 0x41])
/// ```
fn send_bytes_current(_: ActionApi, bytes: array) -> Action;

/// Send a key notation sequence to a specific buffer.
fn send_keys(_: ActionApi, buffer_id: int, notation: string) -> Action;

/// Send a key notation sequence to the focused buffer.
fn send_keys_current(_: ActionApi, notation: string) -> Action;

/// Split the current node and attach the provided tree as the new sibling.
fn split_with(_: ActionApi, direction: string, tree: TreeSpec) -> Action;

/// Toggle a named input mode.
fn toggle_mode(_: ActionApi, mode: string) -> Action;

/// Copy the current selection into the clipboard.
fn yank_selection(_: ActionApi) -> Action;

/// Return the active tab index.
fn active_index(bar: TabBarContext) -> int;

/// Return the active tab index, if any.
///
/// ReturnType: `int | ()`
fn active_tab_index(node: NodeRef) -> ?;

/// Return the current activity state name.
fn activity(buffer: BufferRef) -> String;

/// Return the attached buffer id, if any.
///
/// ReturnType: `int | ()`
fn buffer(node: NodeRef) -> ?;

/// Return how many buffers are attached to the tab.
fn buffer_count(tab: TabInfo) -> int;

/// Return the buffer id attached to an event, or `()`.
///
/// ReturnType: `int | ()`
fn buffer_id(event: EventInfo) -> ?;

/// Return child node ids.
fn children(node: NodeRef) -> array;

/// Return the original command vector.
fn command(buffer: BufferRef) -> array;

/// Return the working directory, if any.
///
/// ReturnType: `string | ()`
fn cwd(buffer: BufferRef) -> ?;

/// Look up a single environment hint captured on the buffer.
///
/// ReturnType: `string | ()`
fn env_hint(buffer: BufferRef, key: string) -> ?;

/// Return the process exit code, if any.
///
/// ReturnType: `int | ()`
fn exit_code(buffer: BufferRef) -> ?;

/// Return floating window ids attached to the session.
fn floating(session: SessionRef) -> array;

/// Return the floating id attached to an event, or `()`.
///
/// ReturnType: `int | ()`
fn floating_id(event: EventInfo) -> ?;

/// Return the floating geometry map.
fn geometry(floating: FloatingRef) -> map;

/// Return the geometry map, if any.
///
/// ReturnType: `Map | ()`
fn geometry(node: NodeRef) -> ?;

/// Return whether the tab has activity.
fn has_activity(tab: TabInfo) -> bool;

/// Return whether the tab has a bell marker.
fn has_bell(tab: TabInfo) -> bool;

/// Return the full captured history text for the buffer.
///
/// # Example
///
/// ```rhai
/// let buffer = ctx.current_buffer();
/// if buffer != () {
///     let history = buffer.history_text();
/// }
/// ```
fn history_text(buffer: BufferRef) -> String;

/// Return the numeric buffer id.
fn id(buffer: BufferRef) -> int;

/// Return the floating id.
fn id(floating: FloatingRef) -> int;

/// Return the node id.
fn id(node: NodeRef) -> int;

/// Return the numeric session id.
fn id(session: SessionRef) -> int;

/// Return the zero-based tab index.
fn index(tab: TabInfo) -> int;

/// Return whether the tab is active.
fn is_active(tab: TabInfo) -> bool;

/// Return whether the buffer is currently attached to a node.
fn is_attached(buffer: BufferRef) -> bool;

/// Return whether the buffer has been detached.
fn is_detached(buffer: BufferRef) -> bool;

/// Return whether the node is the root of a floating window.
fn is_floating_root(node: NodeRef) -> bool;

/// Return whether the floating is focused.
fn is_focused(floating: FloatingRef) -> bool;

/// Return whether the node is focused.
fn is_focused(node: NodeRef) -> bool;

/// Return whether the formatted tabs are the root tabs.
fn is_root(bar: TabBarContext) -> bool;

/// Return whether the node is the session root.
fn is_root(node: NodeRef) -> bool;

/// Return whether the buffer process is still running.
fn is_running(buffer: BufferRef) -> bool;

/// Return whether the buffer is visible in the current presentation.
fn is_visible(buffer: BufferRef) -> bool;

/// Return whether the floating is visible.
fn is_visible(floating: FloatingRef) -> bool;

/// Return whether the node is visible in the current presentation.
fn is_visible(node: NodeRef) -> bool;

/// Return the node kind such as `buffer_view`, `split`, or `tabs`.
fn kind(node: NodeRef) -> String;

/// Return the formatter mode name.
fn mode(bar: TabBarContext) -> String;

/// Return the event name.
fn name(event: EventInfo) -> String;

/// Return the session name.
fn name(session: SessionRef) -> String;

/// Return the tabs node id currently being formatted.
fn node_id(bar: TabBarContext) -> int;

/// Return the attached node id, if any.
///
/// ReturnType: `int | ()`
fn node_id(buffer: BufferRef) -> ?;

/// Return the node id attached to an event, or `()`.
///
/// ReturnType: `int | ()`
fn node_id(event: EventInfo) -> ?;

/// Return the parent node id, if any.
///
/// ReturnType: `int | ()`
fn parent(node: NodeRef) -> ?;

/// Return the process id, if any.
///
/// ReturnType: `int | ()`
fn pid(buffer: BufferRef) -> ?;

/// Return the detected process name, if any.
///
/// ReturnType: `string | ()`
fn process_name(buffer: BufferRef) -> ?;

/// Return the root node id.
fn root_node(floating: FloatingRef) -> int;

/// Return the root tabs node for the session.
fn root_node(session: SessionRef) -> int;

/// Return the attached session id, if any.
///
/// ReturnType: `int | ()`
fn session_id(buffer: BufferRef) -> ?;

/// Return the session id attached to an event, or `()`.
///
/// ReturnType: `int | ()`
fn session_id(event: EventInfo) -> ?;

/// Return the owning session id.
fn session_id(floating: FloatingRef) -> int;

/// Return the owning session id.
fn session_id(node: NodeRef) -> int;

/// Return a text snapshot limited to the requested line count.
fn snapshot_text(buffer: BufferRef, limit: int) -> String;

/// Return the split direction, if any.
///
/// ReturnType: `string | ()`
fn split_direction(node: NodeRef) -> ?;

/// Return split weights, if any.
///
/// ReturnType: `Array | ()`
fn split_weights(node: NodeRef) -> ?;

/// Return tab titles on a tabs node.
fn tab_titles(node: NodeRef) -> array;

/// Return tab metadata used by the formatter.
fn tabs(bar: TabBarContext) -> array;

/// Return the buffer title.
fn title(buffer: BufferRef) -> String;

/// Return the floating title, if any.
///
/// ReturnType: `string | ()`
fn title(floating: FloatingRef) -> ?;

/// Return the tab title.
fn title(tab: TabInfo) -> String;

/// Return the controlling TTY path, if any.
///
/// ReturnType: `string | ()`
fn tty_path(buffer: BufferRef) -> ?;

/// Return the formatter viewport width in cells.
fn viewport_width(bar: TabBarContext) -> int;

/// Return the currently focused buffer, if any.
///
/// ReturnType: `BufferRef | ()`
///
/// # Example
///
/// ```rhai
/// let buffer = ctx.current_buffer();
/// if buffer != () {
///     print(buffer.title());
/// }
/// ```
fn current_buffer(context: Context) -> ?;

/// Return the currently focused floating window, if any.
///
/// ReturnType: `FloatingRef | ()`
fn current_floating(context: Context) -> ?;

/// Return the active input mode name.
fn current_mode(context: Context) -> String;

/// Return the currently focused node, if any.
///
/// ReturnType: `NodeRef | ()`
fn current_node(context: Context) -> ?;

/// Return the current session reference, if any.
///
/// ReturnType: `SessionRef | ()`
fn current_session(context: Context) -> ?;

/// Return detached buffers in the current model snapshot.
fn detached_buffers(context: Context) -> array;

/// Return the current event payload, if any.
///
/// ReturnType: `EventInfo | ()`
fn event(context: Context) -> ?;

/// Find a buffer by numeric id. Returns `()` when it does not exist.
///
/// ReturnType: `BufferRef | ()`
fn find_buffer(context: Context, buffer_id: int) -> ?;

/// Find a floating window by numeric id. Returns `()` when it does not exist.
///
/// ReturnType: `FloatingRef | ()`
fn find_floating(context: Context, floating_id: int) -> ?;

/// Find a node by numeric id. Returns `()` when it does not exist.
///
/// ReturnType: `NodeRef | ()`
fn find_node(context: Context, node_id: int) -> ?;

/// Return every visible session.
fn sessions(context: Context) -> array;

/// Return visible buffers in the current model snapshot.
fn visible_buffers(context: Context) -> array;

let system: SystemApi;

let action: ActionApi;

let tree: TreeApi;

let ui: UiApi;

let theme: ThemeRuntimeApi;

let mux: MuxApi;
