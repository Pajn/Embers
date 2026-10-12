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
///
/// See the overloaded `segment(_: UiApi, text: String, options: Map) -> BarSegment` doc for
/// the full `options: Map` styling keys.
///
/// Its `tab` key tags the segment with one of the bar's tabs, so a left click on it selects
/// that tab, like a click on a window in tmux's status line. Tag every segment a tab is
/// drawn with; untagged segments do nothing when clicked:
///
/// ```rhai
/// for tab in ctx.tabs() {
///     left.push(ui.segment(tab.title(), #{ bold: tab.is_active(), tab: tab }));
/// }
/// ```
fn segment(_: UiApi, text: string) -> BarSegment;

/// Create a [`BarSegment`] from a [`UiApi`] receiver, text, and an `options: Map`.
///
/// See the main `segment(_: UiApi, text: String) -> BarSegment` doc for the shared behavior.
///
/// `segment(_: UiApi, text: String, options: Map) -> BarSegment` supports `fg`, `bg`,
/// `bold`, `italic`, `underline`, `dim`, `blink`, `tab`, and `target` keys to override
/// styling and attach an optional click target.
///
/// `fg` is the foreground color and accepts a standard CSS color name, a hex code such as
/// `#ff0000`, or an RGB/RGBA string such as `rgb(255,0,0)` or `rgba(255,0,0,0.5)`.
/// `bg` is the background color and accepts the same color formats.
/// `bold`, `italic`, `underline`, `dim`, and `blink` are boolean `true`/`false` flags.
/// `dim` renders the text with reduced intensity for a muted appearance.
/// `blink` enables blinking text for that segment, but many modern terminal emulators ignore
/// or disable blink by default, so blinking text may not appear consistently.
/// `tab` tags the segment with a tab, one of the [`TabInfo`] values of the bar's
/// `ctx.tabs()`: with the mouse on (`mouse.set_click_focus`), a left click on the segment
/// selects that tab, as `action.select_tab` does, like a click on a window in tmux's status
/// line. Tag every segment a tab is drawn with, such as its caps, number and name, so a
/// click anywhere on it selects it. Untagged segments do nothing when clicked. The default
/// tab bar needs no tags: a click on a tab's label selects it.
/// `target` is the lower-level form of `tab`, a map naming the tabs node and index:
/// `#{ kind: "tab", tabs_node_id: ctx.node_id(), index: tab.index() }`. A segment takes
/// `tab` or `target`, not both. The `"floating"` and `"buffer"` kinds are accepted but do
/// nothing when clicked.
///
/// Examples:
/// - `#{ fg: "#ff0000", bg: "rgba(0,0,0,0.5)", bold: true }`
/// - `#{ fg: theme.color("base"), bg: theme.color("accent"), tab: tab }`
///
/// Accessibility note: blinking text can be distracting and may trigger seizures for some
/// users. Use `blink` sparingly, prefer non-animated emphasis such as `dim` or color/weight
/// changes, follow WCAG guidance to avoid flashing content, and respect reduced-motion
/// preferences before enabling `blink`.
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

/// Break the current node into a new tab or floating window.
/// `destination` accepts `tab` or `floating`.
/// Example: `action.break_current_node("floating")`.
fn break_current_node(_: ActionApi, destination: string) -> Action;

/// Cancel the active search.
fn cancel_search(_: ActionApi) -> Action;

/// Clear the current selection and return to copy mode with the cursor
/// where the selection ended.
fn cancel_selection(_: ActionApi) -> Action;

/// Chain multiple actions into one composite action.
fn chain(_: ActionApi, actions: array) -> Action;

/// Open the picker on the sessions, collapsed, like tmux `choose-tree -s`
/// (`prefix s`). See `choose_tree` for the keys.
///
/// # Example
///
/// ```rhai
/// bind("normal", "<leader>S", action.choose_session());
/// ```
fn choose_session(_: ActionApi) -> Action;

/// Open the picker on a tree of every session, window and pane, like
/// tmux `choose-tree`. `j`/`k` or the arrows move, `l`/`h` (or Right/Left)
/// expand and collapse, Enter switches to the selected item, `x` kills it
/// after asking, `/` or typing other text filters, and `q`/Esc closes.
///
/// # Example
///
/// ```rhai
/// bind("normal", "<leader>T", action.choose_tree());
/// ```
fn choose_tree(_: ActionApi) -> Action;

/// Open the picker with every session expanded to its windows, like tmux
/// `choose-tree -w` (`prefix w`). See `choose_tree` for the keys.
///
/// # Example
///
/// ```rhai
/// bind("normal", "<leader>\"", action.choose_window());
/// ```
fn choose_window(_: ActionApi) -> Action;

/// Clear any partially-entered key sequence.
fn clear_pending_keys(_: ActionApi) -> Action;

/// Close the currently focused floating window. Its buffers keep running,
/// detached; use `kill_floating` to end them.
fn close_floating(_: ActionApi) -> Action;

/// Close a floating window by id. Its buffers keep running, detached.
fn close_floating_id(_: ActionApi, floating_id: int) -> Action;

/// Close a view by node id. The buffers it shows keep running, detached.
fn close_node(_: ActionApi, node_id: int) -> Action;

/// Close the currently focused view. Its buffer keeps running, detached;
/// use `kill_view` to end it.
fn close_view(_: ActionApi) -> Action;

/// Open the command prompt (tmux `:`). Enter runs the typed line with the
/// same commands and options as the `embers` CLI, such as
/// `split-window --horizontal`, `rename-window logs` or `select-window -t 2`;
/// commands without `-t` apply to the current session, window and pane.
/// Up and Down browse earlier lines; Esc cancels.
///
/// # Example
///
/// ```rhai
/// bind("normal", "<leader>:", action.command_prompt());
/// ```
fn command_prompt(_: ActionApi) -> Action;

/// Open a prompt labelled `prompt` with `initial` typed (both may use
/// formats); Enter runs `template` with `%%` replaced by the input as one
/// word, like tmux `command-prompt -p prompt -I initial template`.
///
/// # Example
///
/// ```rhai
/// action.command_prompt_template("(new window) ", "", "new-window --title %%")
/// ```
fn command_prompt_template(_: ActionApi, prompt: string, initial: string, template: string) -> Action;

/// Open the command prompt with `initial` already typed, like tmux
/// `command-prompt -I`. The text may use the formats `display_message`
/// accepts, such as `#W`.
///
/// # Example
///
/// ```rhai
/// action.command_prompt_with("split-window -- ")
/// ```
fn command_prompt_with(_: ActionApi, initial: string) -> Action;

/// Finalize the active search, keep the current match and cursor position, and leave search
/// mode with the committed result in place.
fn commit_search(_: ActionApi) -> Action;

/// Ask before running `action`, like tmux `confirm-before`: `prompt` is
/// shown in the status line, expanded as a format (`#W`, `#D`, ...), and
/// `action`, an action or an array of actions, runs only if the next key
/// is `y`. Any other key cancels.
///
/// # Example
///
/// ```rhai
/// bind("normal", "<leader>x", action.confirm("kill-pane #D? (y/n)", action.run_command("kill-pane")));
/// ```
fn confirm(_: ActionApi, prompt: string, action: ?) -> Action;

/// Move to the first non-blank character of the line (`^`).
fn copy_back_to_indentation(_: ActionApi) -> Action;

/// Move to the bottom row of the view (`L`).
fn copy_bottom_line(_: ActionApi) -> Action;

/// Move the copy-mode cursor down (`j`).
fn copy_cursor_down(_: ActionApi) -> Action;

/// Move the copy-mode cursor left (`h`).
fn copy_cursor_left(_: ActionApi) -> Action;

/// Move the copy-mode cursor right (`l`).
fn copy_cursor_right(_: ActionApi) -> Action;

/// Move the copy-mode cursor up (`k`).
fn copy_cursor_up(_: ActionApi) -> Action;

/// Move to the last character of the line (`$`).
fn copy_end_of_line(_: ActionApi) -> Action;

/// Scroll the view and move the cursor down half a page (`C-d`).
fn copy_halfpage_down(_: ActionApi) -> Action;

/// Scroll the view and move the cursor up half a page (`C-u`).
fn copy_halfpage_up(_: ActionApi) -> Action;

/// Move to the last line of the buffer (`G`).
fn copy_history_bottom(_: ActionApi) -> Action;

/// Move to the first line of history (`gg`).
fn copy_history_top(_: ActionApi) -> Action;

/// Repeat the last character jump (`;`).
fn copy_jump_again(_: ActionApi) -> Action;

/// Wait for a character and move onto its previous occurrence on the line
/// (`F`).
fn copy_jump_backward(_: ActionApi) -> Action;

/// Wait for a character and move onto its next occurrence on the line (`f`).
fn copy_jump_forward(_: ActionApi) -> Action;

/// Repeat the last character jump in the opposite direction (`,`).
fn copy_jump_reverse(_: ActionApi) -> Action;

/// Wait for a character and move just after its previous occurrence on the
/// line (`T`).
fn copy_jump_to_backward(_: ActionApi) -> Action;

/// Wait for a character and move just before its next occurrence on the
/// line (`t`).
fn copy_jump_to_forward(_: ActionApi) -> Action;

/// Move to the middle row of the view (`M`).
fn copy_middle_line(_: ActionApi) -> Action;

/// Move to the start of the next whitespace-separated word (`W`).
fn copy_next_space(_: ActionApi) -> Action;

/// Move to the end of the next whitespace-separated word (`E`).
fn copy_next_space_end(_: ActionApi) -> Action;

/// Move to the start of the next word (`w`). Whitespace separates words, and
/// a run of ASCII punctuation other than `_` is a word of its own.
fn copy_next_word(_: ActionApi) -> Action;

/// Move to the end of the next word (`e`).
fn copy_next_word_end(_: ActionApi) -> Action;

/// Scroll the view and move the cursor down a page (`C-f`).
fn copy_page_down(_: ActionApi) -> Action;

/// Scroll the view and move the cursor up a page (`C-b`).
fn copy_page_up(_: ActionApi) -> Action;

/// Move to the start of the previous whitespace-separated word (`B`).
fn copy_previous_space(_: ActionApi) -> Action;

/// Move to the start of the previous word (`b`).
fn copy_previous_word(_: ActionApi) -> Action;

/// Copy the current selection into the clipboard (OSC 52) and leave copy
/// mode. Same as `yank_selection`.
fn copy_selection(_: ActionApi) -> Action;

/// Move to the first column of the line (`0`).
fn copy_start_of_line(_: ActionApi) -> Action;

/// Move to the top row of the view (`H`).
fn copy_top_line(_: ActionApi) -> Action;

/// Detach the currently focused buffer.
fn detach_buffer(_: ActionApi) -> Action;

/// Detach a buffer by id.
fn detach_buffer_id(_: ActionApi, buffer_id: int) -> Action;

/// Show a message in the status line for a few seconds, like tmux
/// `display-message`. The text may use the format variables `#S` (session),
/// `#I` (window number), `#W` (window name), `#D` (pane id), `#T` (pane
/// title), `#{pane_current_path}`, `#{pane_current_command}` and
/// `#{@name}` (the pane's buffer option `name`), expanded for the focused
/// pane; `##` is a literal `#`.
///
/// # Example
///
/// ```rhai
/// action.display_message("#S:#I #{pane_current_path}")
/// ```
fn display_message(_: ActionApi, message: string) -> Action;

/// Show a message in the status line for `duration_ms` milliseconds, like
/// tmux `display-message -d`. See `display_message(message)` for formats.
///
/// # Example
///
/// ```rhai
/// action.display_message("saved", 1000)
/// ```
fn display_message(_: ActionApi, message: string, duration_ms: int) -> Action;

/// Enter copy mode on the focused pane (tmux `copy-mode`). The view stops
/// following output and a cursor appears at the program's cursor; copy-mode
/// motions move it. Does nothing on the alternate screen.
fn enter_copy_mode(_: ActionApi) -> Action;

/// Enter thumbs-style hint mode: label the matches in the visible pane and
/// copy the selected one to the clipboard (OSC 52).
///
/// # Example
///
/// ```rhai
/// action.enter_hints()
/// ```
fn enter_hints(_: ActionApi) -> Action;

/// Enter hint mode, invoking the named action with the selected text exposed
/// as `ctx.hint_selection()` instead of copying it.
///
/// # Example
///
/// ```rhai
/// action.enter_hints_with("open-url")
/// ```
fn enter_hints_with(_: ActionApi, action: string) -> Action;

/// Enter a specific input mode by name.
fn enter_mode(_: ActionApi, mode: string) -> Action;

/// Enter incremental search mode, searching backward (`?`). After a backward
/// search `search_next` moves to earlier matches and `search_prev` to later
/// ones, as in vim.
fn enter_search_backward(_: ActionApi) -> Action;

/// Enter incremental search mode, searching forward (`/`).
fn enter_search_mode(_: ActionApi) -> Action;

/// Start a block (rectangle) selection at the copy-mode cursor. Toggles
/// like `enter_select_char`.
fn enter_select_block(_: ActionApi) -> Action;

/// Start a character selection at the copy-mode cursor, entering copy mode
/// first if needed. With a character selection already active this clears
/// it; with another kind active it switches to character selection.
fn enter_select_char(_: ActionApi) -> Action;

/// Start a line selection at the copy-mode cursor. Toggles like
/// `enter_select_char`.
fn enter_select_line(_: ActionApi) -> Action;

/// Leave copy mode: drop the cursor and any selection, follow output again,
/// and return to `normal` mode (tmux `cancel`).
fn exit_copy_mode(_: ActionApi) -> Action;

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

/// Insert a tab after the active tab of the innermost tabs that hold the
/// focus, and select it. When the focus is inside nested tabs, the new tab
/// is a nested tab there, not a window; use `new_window` for tmux's
/// `new-window`. In a session with a single window, whose root is not
/// tabbed, the new tab becomes a second window after that one.
///
/// # Example
///
/// ```rhai
/// action.insert_tab_after_current("logs", tree.buffer_spawn(["tail", "-f", "app.log"]))
/// ```
fn insert_tab_after_current(_: ActionApi, title: string, tree: TreeSpec) -> Action;

/// Insert a tab before a specific tabs node.
fn insert_tab_before(_: ActionApi, tabs_node_id: int, title: string, tree: TreeSpec) -> Action;

/// Insert a tab before the active tab of the innermost tabs that hold the
/// focus, and select it. Like `insert_tab_after_current`, this makes a
/// nested tab when the focus is in nested tabs, and a second window, before
/// the only one, in a session with a single window.
fn insert_tab_before_current(_: ActionApi, title: string, tree: TreeSpec) -> Action;

/// Attach a buffer at the current node.
/// `placement` accepts `tab-after`, `tab-before`, `left`, `right`, `up`, or `down`.
/// Example: `action.join_buffer_here(12, "tab-after")`.
fn join_buffer_here(_: ActionApi, buffer_id: int, placement: string) -> Action;

/// Kill the currently focused buffer.
fn kill_buffer(_: ActionApi) -> Action;

/// Kill a buffer by id.
fn kill_buffer_id(_: ActionApi, buffer_id: int) -> Action;

/// Close the currently focused floating window and end the programs of the
/// buffers it shows, like tmux's `kill-pane` for a popup.
fn kill_floating(_: ActionApi) -> Action;

/// Close a floating window by id and end the programs of the buffers it
/// shows.
fn kill_floating_id(_: ActionApi, floating_id: int) -> Action;

/// Close a view by node id and end the programs of the buffers it shows. On
/// a split or tabs node this ends every pane in it, like tmux's
/// `kill-window`.
fn kill_node(_: ActionApi, node_id: int) -> Action;

/// Close the currently focused view and end its buffer's program, like
/// tmux's `kill-pane`.
fn kill_view(_: ActionApi) -> Action;

/// Focus the pane that was focused before the current one in the current
/// window, like tmux `select-pane -l`. Repeating it goes back and forth
/// between two panes. Panes that were closed are skipped; with no other
/// pane to go back to it does nothing.
///
/// # Example
///
/// ```rhai
/// bind("normal", "<leader>;", action.last_pane());
/// ```
fn last_pane(_: ActionApi) -> Action;

/// Switch back to the previously active session (tmux `switch-client -l`).
fn last_session(_: ActionApi) -> Action;

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

/// Move the current node after a sibling.
fn move_current_node_after(_: ActionApi, sibling_node_id: int) -> Action;

/// Move the current node before a sibling.
/// Use this when the current node is the one being repositioned.
/// Example: `action.move_current_node_before(42)`.
fn move_current_node_before(_: ActionApi, sibling_node_id: int) -> Action;

/// Move a node after a sibling.
/// Use this when you need to move a specific node id instead of the current node.
/// Example: `action.move_node_after(10, 42)`.
fn move_node_after(_: ActionApi, node_id: int, sibling_node_id: int) -> Action;

/// Move a node before a sibling.
fn move_node_before(_: ActionApi, node_id: int, sibling_node_id: int) -> Action;

/// Add a window to the current session, after its last window, and
/// select it (tmux `new-window`). Unlike `insert_tab_after_current`, the
/// new tab is always a window, even when the focus is in nested tabs.
///
/// # Example
///
/// ```rhai
/// action.new_window("logs", tree.buffer_spawn(["tail", "-f", "app.log"]))
/// ```
fn new_window(_: ActionApi, title: string, tree: TreeSpec) -> Action;

/// Select the next tab in the currently focused tabs node.
fn next_current_tabs(_: ActionApi) -> Action;

/// Switch to the next session in list order, wrapping around.
fn next_session(_: ActionApi) -> Action;

/// Select the next tab in a specific tabs node.
fn next_tab(_: ActionApi, tabs_node_id: int) -> Action;

/// Build a no-op action.
fn noop(_: ActionApi) -> Action;

/// Emit a client notification.
fn notify(_: ActionApi, level: string, message: string) -> Action;

/// Open the history of a buffer in a new view.
/// `scope` accepts `visible` or `full`. `placement` accepts `floating` or `tab`.
/// Example: `action.open_buffer_history(12, "visible", "floating")`.
fn open_buffer_history(_: ActionApi, buffer_id: int, scope: string, placement: string) -> Action;

/// Open a floating view around the provided tree.
fn open_floating(_: ActionApi, tree: TreeSpec, options: map) -> Action;

/// Select the previous tab in the currently focused tabs node.
fn prev_current_tabs(_: ActionApi) -> Action;

/// Switch to the previous session in list order, wrapping around.
fn prev_session(_: ActionApi) -> Action;

/// Select the previous tab in a specific tabs node.
fn prev_tab(_: ActionApi, tabs_node_id: int) -> Action;

/// Prompt for a new name for the current session, starting from its
/// current name (tmux `prefix $`).
///
/// # Example
///
/// ```rhai
/// bind("normal", "<leader>$", action.rename_session_prompt());
/// ```
fn rename_session_prompt(_: ActionApi) -> Action;

/// Prompt for a new name for the current window, starting from its current
/// name (tmux `prefix ,`).
///
/// # Example
///
/// ```rhai
/// bind("normal", "<leader>,", action.rename_window_prompt());
/// ```
fn rename_window_prompt(_: ActionApi) -> Action;

/// Replace the focused node with a new tree.
fn replace_current_with(_: ActionApi, tree: TreeSpec) -> Action;

/// Replace a specific node by id with a new tree.
fn replace_node(_: ActionApi, node_id: int, tree: TreeSpec) -> Action;

/// Move the focused pane's border `cells` rows down (tmux `resize-pane -D`).
/// See `resize_left` for which divider moves.
fn resize_down(_: ActionApi, cells: int) -> Action;

/// Move the focused pane's border `cells` cells left (tmux `resize-pane -L`).
///
/// The nearest split with panes side by side is resized: the divider to the right of the
/// pane moves left, shrinking the pane, or, when the pane is the split's rightmost child,
/// the divider to its left moves left, growing it. Panes keep at least one column of
/// content, and the action does nothing when there is no such split or the divider cannot
/// move. While a pane is zoomed only splits inside the zoomed view can be resized.
///
/// # Example
///
/// ```rhai
/// bind("normal", "<M-h>", action.resize_left(3));
/// ```
fn resize_left(_: ActionApi, cells: int) -> Action;

/// Move the focused pane's border `cells` cells right (tmux `resize-pane -R`).
/// See `resize_left` for which divider moves.
fn resize_right(_: ActionApi, cells: int) -> Action;

/// Move the focused pane's border `cells` rows up (tmux `resize-pane -U`).
/// See `resize_left` for which divider moves; panes keep their title row and at least
/// one row of content.
fn resize_up(_: ActionApi, cells: int) -> Action;

/// Reveal a specific buffer by id.
fn reveal_buffer(_: ActionApi, buffer_id: int) -> Action;

/// Run a command line as if it was typed at the command prompt, like
/// binding a tmux command. Output and errors are shown in the status line.
///
/// # Example
///
/// ```rhai
/// bind("normal", "<leader>E", action.run_command("set-environment -g EDITOR nvim"));
/// ```
fn run_command(_: ActionApi, command: string) -> Action;

/// Run another named action by name.
fn run_named_action(_: ActionApi, name: string) -> Action;

/// Run a shell command line in the client's login context (tmux `run-shell`).
///
/// The string is executed as `/bin/sh -lc "<command>"`. Output is discarded;
/// the spawned tool can drive Embers via `$EMBERS_SOCKET` and the `embers` CLI.
/// `$EMBERS_CLIENT` and `$EMBERS_BUFFER` name the client the binding was
/// pressed in and its focused buffer.
///
/// # Example
///
/// ```rhai
/// action.run_shell("wisp popup")
/// ```
fn run_shell(_: ActionApi, command: string) -> Action;

/// Run a command given as an explicit argv array, without a shell.
///
/// # Example
///
/// ```rhai
/// action.run_shell_argv(["wisp", "popup"])
/// ```
fn run_shell_argv(_: ActionApi, argv: array) -> Action;

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

/// Move the copy-mode cursor down. Same as `copy_cursor_down`.
fn select_move_down(_: ActionApi) -> Action;

/// Move the copy-mode cursor left. Same as `copy_cursor_left`.
fn select_move_left(_: ActionApi) -> Action;

/// Move the copy-mode cursor right. Same as `copy_cursor_right`.
fn select_move_right(_: ActionApi) -> Action;

/// Move the copy-mode cursor up. Same as `copy_cursor_up`.
fn select_move_up(_: ActionApi) -> Action;

/// Swap the selection's ends, so motions move the other end (`o`).
fn select_other_end(_: ActionApi) -> Action;

/// Select a tab by index in a specific tabs node.
fn select_tab(_: ActionApi, tabs_node_id: int, index: int) -> Action;

/// Select a window (a root tab of the current session) by its number, which
/// counts from the base index set with `set_base_index` (0 when unset), like
/// tmux `select-window -t :N`.
///
/// # Example
///
/// ```rhai
/// bind("normal", "<leader>1", action.select_window(1));
/// ```
fn select_window(_: ActionApi, number: int) -> Action;

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

/// Set an environment variable for panes created later in the current
/// session (tmux `set-environment`). Pass `()` as the value to unset it.
/// Panes that already run keep their environment.
///
/// # Example
///
/// ```rhai
/// action.set_environment("AWS_PROFILE", "staging")
/// ```
fn set_environment(_: ActionApi, name: string, value: ?) -> Action;

/// Set an environment variable for panes created later in every session
/// (tmux `set-environment -g`). Pass `()` as the value to unset it. A
/// session's own value wins over the global one.
///
/// # Example
///
/// ```rhai
/// action.set_global_environment("EDITOR", "nvim")
/// ```
fn set_global_environment(_: ActionApi, name: string, value: ?) -> Action;

/// Turn tmux-style `synchronize-panes` on or off for the active window.
///
/// The setting belongs to the window (a root tab) and is shared by every client. While it
/// is on, keys, pastes and `send_keys_current`/`send_bytes_current` input aimed at the
/// focused pane go to every pane the window shows, each encoded for that pane. Panes in
/// hidden nested tabs or hidden by zoom, panes in other windows, and popups are left out,
/// and input typed into a focused popup is never copied. Mouse and focus events, input
/// sent to a specific buffer id, and `embers send-keys` are not synchronized. Tab bars can
/// show the state through `TabInfo.synchronize_panes()`.
fn set_synchronize_panes(_: ActionApi, enabled: bool) -> Action;

/// Split the current node and attach the provided tree as the new sibling.
fn split_with(_: ActionApi, direction: string, tree: TreeSpec) -> Action;

/// Swap the current node with a sibling.
fn swap_current_node(_: ActionApi, sibling_node_id: int) -> Action;

/// Switch the client to the session with the given name (tmux `switch-client -t`).
///
/// # Example
///
/// ```rhai
/// action.switch_session("work")
/// ```
fn switch_session(_: ActionApi, name: string) -> Action;

/// Toggle a named input mode.
fn toggle_mode(_: ActionApi, mode: string) -> Action;

/// Toggle tmux-style `synchronize-panes` for the active window.
///
/// While it is on, input typed into a pane of the window is also sent to every other pane
/// the window currently shows. Popups and other windows never take part. See
/// `set_synchronize_panes` for details.
///
/// # Example
///
/// ```rhai
/// bind("normal", "<leader>S", action.toggle_synchronize_panes());
/// ```
fn toggle_synchronize_panes(_: ActionApi) -> Action;

/// Toggle zoom for the focused node (tmux `resize-pane -Z`).
///
/// # Example
///
/// ```rhai
/// bind("normal", "<leader>z", action.toggle_zoom_current_node());
/// ```
fn toggle_zoom_current_node(_: ActionApi) -> Action;

/// Toggle zoom for the specified node id.
fn toggle_zoom_node(_: ActionApi, node_id: int) -> Action;

/// Clear the current session's active zoom state.
/// This removes the current session zoom rather than unwinding a stack of prior zooms.
fn unzoom_current_session(_: ActionApi) -> Action;

/// Copy the current selection into the clipboard (OSC 52) and leave copy
/// mode. Without a selection it only leaves copy mode.
fn yank_selection(_: ActionApi) -> Action;

/// Zoom the session's currently focused node.
/// There is intentionally no separate `zoom_node(node_id)` helper in this API surface.
fn zoom_current_node(_: ActionApi) -> Action;

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

/// Return the client id attached to an event, or `()`.
///
/// ReturnType: `int | ()`
fn client_id(event: EventInfo) -> ?;

/// Return the original command vector.
fn command(buffer: BufferRef) -> array;

/// Return the working directory, if any.
///
/// ReturnType: `string | ()`
fn cwd(buffer: BufferRef) -> ?;

/// Return the working directory of the pane the tab shows (see
/// `pane_title`), like tmux's `#{pane_current_path}`.
///
/// ReturnType: `string | ()`
fn cwd(tab: TabInfo) -> ?;

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

/// Return the command line of the program in the buffer's foreground: the
/// job its shell runs, else the buffer's own command.
fn foreground_command(buffer: BufferRef) -> array;

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

/// Return the zero-based tab index, as `action.select_tab` takes it.
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

/// Return the tab's window number: its index plus the base index set with
/// `set_base_index` (0 when unset), like tmux `#I`. Show this in tab bars so
/// the numbers match CLI targets such as `select-window -t work:2` and
/// `action.select_window`.
///
/// # Example
///
/// ```rhai
/// let label = `${tab.number()}:${tab.title()}`;
/// ```
fn number(tab: TabInfo) -> int;

/// Return the title of the pane the tab shows: the focused pane when the
/// tab holds the focus, otherwise the pane its active nested tabs lead to
/// (the first pane of a split).
///
/// ReturnType: `string | ()`
fn pane_title(tab: TabInfo) -> ?;

/// Return the parent node id, if any.
///
/// ReturnType: `int | ()`
fn parent(node: NodeRef) -> ?;

/// Return the process id, if any.
///
/// ReturnType: `int | ()`
fn pid(buffer: BufferRef) -> ?;

/// Return the previous session id attached to an event, or `()`.
///
/// ReturnType: `int | ()`
fn previous_session_id(event: EventInfo) -> ?;

/// Return the name of the program in the buffer's foreground, like tmux's
/// `pane_current_command`: the job its shell runs (`"nvim"` while Neovim
/// runs in a shell), else the buffer's own command (`"zsh"` at the
/// prompt). Use `command()` for the command the buffer was started with.
///
/// ReturnType: `string | ()`
///
/// # Example
///
/// ```rhai
/// let buffer = ctx.current_buffer();
/// if buffer != () && buffer.process_name() == "nvim" {
///     action.send_keys_current("<C-h>")
/// }
/// ```
fn process_name(buffer: BufferRef) -> ?;

/// Return the name of the program in the foreground of the pane the tab
/// shows (see `pane_title`), like tmux's `#{pane_current_command}`.
///
/// ReturnType: `string | ()`
///
/// # Example
///
/// ```rhai
/// let label = if tab.process_name() == "nvim" { "editor" } else { tab.title() };
/// ```
fn process_name(tab: TabInfo) -> ?;

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

/// Return the name of the session the bar belongs to, like tmux `#S`, for
/// a status line such as `[main] 1 shell 2 nvim`.
///
/// # Example
///
/// ```rhai
/// let left = [ui.segment(`[${ctx.session_name()}] `)];
/// ```
fn session_name(bar: TabBarContext) -> String;

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

/// Return whether `synchronize-panes` is on for the window this tab holds, so a formatter
/// can flag it. Always `false` for nested tabs.
///
/// # Example
///
/// ```rhai
/// let label = if tab.synchronize_panes() { tab.title() + " [sync]" } else { tab.title() };
/// ```
fn synchronize_panes(tab: TabInfo) -> bool;

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

/// Look up a runtime user option set on the buffer via `embers buffer set-option`.
///
/// ReturnType: `string | ()`
fn user_option(buffer: BufferRef, key: string) -> ?;

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

/// Return the text selected in hint mode, when a hint callback is running.
///
/// ReturnType: `string | ()`
fn hint_selection(context: Context) -> ?;

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
