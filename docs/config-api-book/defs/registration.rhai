module static;



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

/// Toggle focus-on-click behavior.
///
/// # rhai-autodocs:index:22
fn set_click_focus(mouse: MouseApi, value: bool) -> ();

/// Toggle forwarding mouse clicks into the focused buffer.
///
/// # rhai-autodocs:index:23
fn set_click_forward(mouse: MouseApi, value: bool) -> ();

/// Toggle wheel event forwarding into the focused buffer.
///
/// # rhai-autodocs:index:25
fn set_wheel_forward(mouse: MouseApi, value: bool) -> ();

/// Toggle client-side wheel scrolling.
///
/// # rhai-autodocs:index:24
fn set_wheel_scroll(mouse: MouseApi, value: bool) -> ();

/// Add named colors to the theme palette.
///
/// # rhai-autodocs:index:21
fn set_palette(theme: ThemeApi, palette: map) -> ();

/// Register the function used to format the tab bar.
///
/// # rhai-autodocs:index:20
fn set_formatter(tabbar: TabbarApi, callback: Fn) -> ();

fn bind(mode: string, notation: string, action: Action) -> ();

/// Bind a key notation to an [`Action`], a string action name, or an array of actions.
///
/// Use the `Action` overload for inline builders such as `action.focus_left()`, the string
/// overload for a named action registered with `define_action`, or an array to chain multiple
/// actions in sequence.
///
/// # Example
///
/// ```rhai
/// bind("normal", "<leader>ws", "workspace-split");
/// ```
///
/// # rhai-autodocs:index:4
fn bind(mode: string, notation: string, action_name: string) -> ();

fn bind(mode: string, notation: string, actions: array) -> ();

/// Register a function pointer as a named action callable from bindings.
///
/// # rhai-autodocs:index:6
fn define_action(name: string, callback: Fn) -> ();

fn define_mode(mode_name: string) -> ();

/// Define a custom input mode with hooks and fallback options.
///
/// Supported options are `fallback`, `on_enter`, and `on_leave`.
///
/// # rhai-autodocs:index:3
fn define_mode(mode_name: string, options: map) -> ();

/// Attach a callback to an emitted event such as `buffer_bell`.
///
/// # rhai-autodocs:index:7
fn on(event_name: string, callback: Fn) -> ();

/// Set the leader sequence used in binding notations.
///
/// # Example
///
/// ```rhai
/// set_leader("<C-a>");
/// ```
///
/// # rhai-autodocs:index:1
fn set_leader(notation: string) -> ();

/// Remove a previously bound key sequence.
///
/// # rhai-autodocs:index:5
fn unbind(mode: string, notation: string) -> ();

let system: SystemApi;

let action: ActionApi;

let tree: TreeApi;

let ui: UiApi;

let tabbar: TabbarApi;

let theme: ThemeApi;

let mouse: MouseApi;
