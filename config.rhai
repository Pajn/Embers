// Example Embers config: a typical power-user tmux setup, translated.
//
// Try it with `embers --config ./config.rhai`, or copy it to
// ~/.config/embers/config.rhai. Each section names the tmux settings it
// replaces; settings with no Embers equivalent are listed with the reason.
//
// Starting: `embers` attaches to the most recent session, or creates one
// named `main`, which covers the usual `new-session -A` habit. For a fixed
// name: `embers new-session 0; embers attach -t 0`.

// ---------------------------------------------------------------------------
// Terminal
// ---------------------------------------------------------------------------
// default-terminal: panes get TERM=xterm-256color and COLORTERM=truecolor,
//   and Embers answers the queries programs use to find curly underlines and
//   OSC 52 (see docs/configuration.md#terminal-capabilities).
// extended-keys / extended-keys-format csi-u: the client enables the kitty
//   keyboard protocol on the host terminal, and panes that ask for it get
//   CSI-u keys. Nothing to set.
// escape-time 0: Esc is told apart from Alt+key within 15 ms, not settable.
// focus-events on: always on for programs that enable focus reporting.
// history-limit 100000: a server setting, EMBERS_MAX_SCROLLBACK_LINES=100000
//   in the environment the server starts in (the default is 10000).
// set-titles on + set-titles-string '#S': built in; the host terminal title
//   is the session name.

set_clipboard(true);                 // set-clipboard on (OSC 52, the default)
set_allow_passthrough("on");         // allow-passthrough on (images, notifications)
set_detach_on_destroy("off");        // detach-on-destroy off

// ---------------------------------------------------------------------------
// Numbering
// ---------------------------------------------------------------------------
// base-index 1. The client reports it to the server, so `embers` commands run
// in shells agree. Windows are always renumbered (renumber-windows on). Panes
// are addressed by id, so there is no pane-base-index.
set_base_index(1);

// ---------------------------------------------------------------------------
// Prefix
// ---------------------------------------------------------------------------
// prefix C-a; `bind C-a send-prefix`. There is no default prefix to unbind.
set_leader("<C-a>");
bind("normal", "<leader><C-a>", action.send_keys_current("<C-a>"));

// ---------------------------------------------------------------------------
// Mouse, copy mode and hints
// ---------------------------------------------------------------------------
// mouse on is the built-in default: click to focus, wheel scrolling, drag to
// select and copy, dragging split dividers. mode-keys vi is the built-in copy
// mode table, where v, C-v and y already do begin-selection,
// rectangle-toggle and copy-selection-and-cancel.
set_status_keys("vi");               // status-keys vi
bind("normal", "<leader>[", action.enter_copy_mode());

// tmux-thumbs: label URLs, paths, hashes and numbers on screen and copy the
// chosen one to the clipboard with OSC 52.
bind("normal", "<leader><Space>", action.enter_hints());

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------
// `action`, `tree`, `system`, `theme` and `ui` are only visible in the
// functions Embers calls directly, so helpers take plain values.

// Spawn options for a pane titled `title` in the focused pane's directory,
// like `-c "#{pane_current_path}"`.
fn here(ctx, title) {
    let buffer = ctx.current_buffer();
    #{ title: title, cwd: if buffer == () { () } else { buffer.cwd() } }
}

// The user's shell, from `system.env("SHELL")`.
fn shell_command(shell) {
    if shell == () { ["sh"] } else { [shell] }
}

// display-popup -EE -w 90% -h 90%: the popup closes when its program exits.
fn popup_options(title) {
    #{ title: title, width: "90%", height: "90%" }
}

// The session's windows are the tabs of its root node; with one window the
// root is not tabbed.
fn window_tabs(ctx) {
    let session = ctx.current_session();
    if session == () {
        return ();
    }
    let root = ctx.find_node(session.root_node());
    if root == () || root.kind() != "tabs" { () } else { root.id() }
}

// ---------------------------------------------------------------------------
// Windows and panes
// ---------------------------------------------------------------------------
// tmux `split-window -h` puts panes side by side, which Embers calls a
// "vertical" split (a vertical divider); `-v` stacks them ("horizontal").
fn split_right(ctx) {
    let shell = tree.buffer_spawn(shell_command(system.env("SHELL")), here(ctx, "shell"));
    action.split_with("vertical", shell)
}

fn split_below(ctx) {
    let shell = tree.buffer_spawn(shell_command(system.env("SHELL")), here(ctx, "shell"));
    action.split_with("horizontal", shell)
}

fn new_window(ctx) {
    let shell = tree.buffer_spawn(shell_command(system.env("SHELL")), here(ctx, "shell"));
    action.new_window("shell", shell)
}

fn file_manager_window(ctx) {
    action.new_window("yazi", tree.buffer_spawn(["yazi"], here(ctx, "yazi")))
}

fn next_window(ctx) {
    let tabs = window_tabs(ctx);
    if tabs == () { action.noop() } else { action.next_tab(tabs) }
}

fn previous_window(ctx) {
    let tabs = window_tabs(ctx);
    if tabs == () { action.noop() } else { action.prev_tab(tabs) }
}

define_action("split-right", split_right);
define_action("split-below", split_below);
define_action("new-window", new_window);
define_action("file-manager-window", file_manager_window);
define_action("next-window", next_window);
define_action("previous-window", previous_window);

bind("normal", "<leader>v", "split-right");
bind("normal", "<leader>s", "split-below");
bind("normal", "<leader>c", "new-window");
bind("normal", "<leader>b", "file-manager-window");

// Window switching without the prefix. C-Tab and C-S-Tab (C-BTab) only reach
// Embers from a host terminal with the kitty keyboard protocol; most others
// send them as plain Tab and Shift-Tab.
bind("normal", "<S-Left>", "previous-window");
bind("normal", "<S-Right>", "next-window");
bind("normal", "<M-H>", "previous-window");
bind("normal", "<M-L>", "next-window");
bind("normal", "<C-Tab>", "next-window");
bind("normal", "<C-S-Tab>", "previous-window");

// tmux defaults that Embers does not bind on its own.
bind("normal", "<leader>n", "next-window");
bind("normal", "<leader>p", "previous-window");
for number in 1..10 {
    bind("normal", `<leader>${number}`, action.select_window(number));
}
bind("normal", "<leader>z", action.toggle_zoom_current_node());
bind("normal", "<leader>x", action.confirm("kill-pane #D? (y/n)", action.run_command("kill-pane")));
bind("normal", "<leader>&", action.confirm("kill-window #W? (y/n)", action.run_command("kill-window")));
bind("normal", "<leader>:", action.command_prompt());
bind("normal", "<leader>,", action.rename_window_prompt());
bind("normal", "<leader>$", action.rename_session_prompt());

// Pickers and sessions.
bind("normal", "<leader>\"", action.choose_window());
bind("normal", "<leader>'", action.last_session());  // switch-client -l

// ---------------------------------------------------------------------------
// Popups and tools
// ---------------------------------------------------------------------------
fn lazygit_popup(ctx) {
    let lazygit = tree.buffer_spawn(["lazygit"], here(ctx, "lazygit"));
    action.open_floating(lazygit, popup_options("lazygit"))
}

fn git_log_popup(ctx) {
    let git_log = tree.buffer_spawn(["sh", "-lc", "git log | nvim -R -"], here(ctx, "git log"));
    action.open_floating(git_log, popup_options("git log"))
}

fn shell_popup(ctx) {
    let shell = tree.buffer_spawn(shell_command(system.env("SHELL")), here(ctx, "shell"));
    action.open_floating(shell, popup_options("shell"))
}

define_action("lazygit-popup", lazygit_popup);
define_action("git-log-popup", git_log_popup);
define_action("shell-popup", shell_popup);

bind("normal", "<leader>g", "lazygit-popup");
bind("normal", "<leader>l", "git-log-popup");
bind("normal", "<leader>d", "shell-popup");

// `run-shell` runs a command line with `sh -lc` and discards its output; the
// command can drive Embers through the `embers` CLI. For example:
// bind("normal", "<leader>o", action.run_shell("my-session-tool popup"));

// ---------------------------------------------------------------------------
// History
// ---------------------------------------------------------------------------
// The pane's visible screen (h) or whole scrollback (H) as a read-only copy
// in a new tab. Scroll and search it with the copy mode keys, as in a pager.
fn visible_history(ctx) {
    let buffer = ctx.current_buffer();
    if buffer == () { action.noop() } else { action.open_buffer_history(buffer.id(), "visible", "tab") }
}

fn full_history(ctx) {
    let buffer = ctx.current_buffer();
    if buffer == () { action.noop() } else { action.open_buffer_history(buffer.id(), "full", "tab") }
}

define_action("visible-history", visible_history);
define_action("full-history", full_history);

bind("normal", "<leader>h", "visible-history");
bind("normal", "<leader>H", "full-history");

// ---------------------------------------------------------------------------
// smart-splits.nvim
// ---------------------------------------------------------------------------
// C-h/j/k/l move between panes and M-h/j/k/l resize them by 3 cells, unless
// the pane runs Neovim, which gets the key instead. Neovim can mark its pane
// with `embers buffer set-option is-vim 1`, the counterpart of @pane-is-vim;
// see docs/layout-controls.md#smart-splitsnvim for the autocommands.
fn is_vim_pane(ctx) {
    let buffer = ctx.current_buffer();
    let node = ctx.current_node();
    if buffer == () || node == () || node.kind() != "buffer_view" {
        return false;
    }
    let is_vim = buffer.user_option("is-vim");
    if is_vim == "1" {
        return true;
    }
    if is_vim == "0" {
        return false;
    }
    let name = buffer.process_name();
    name == "nvim" || name == "vim"
}

fn smart_nav_left(ctx) { if is_vim_pane(ctx) { action.send_keys_current("<C-h>") } else { action.focus_left() } }
fn smart_nav_down(ctx) { if is_vim_pane(ctx) { action.send_keys_current("<C-j>") } else { action.focus_down() } }
fn smart_nav_up(ctx) { if is_vim_pane(ctx) { action.send_keys_current("<C-k>") } else { action.focus_up() } }
fn smart_nav_right(ctx) { if is_vim_pane(ctx) { action.send_keys_current("<C-l>") } else { action.focus_right() } }

fn smart_resize_left(ctx) { if is_vim_pane(ctx) { action.send_keys_current("<M-h>") } else { action.resize_left(3) } }
fn smart_resize_down(ctx) { if is_vim_pane(ctx) { action.send_keys_current("<M-j>") } else { action.resize_down(3) } }
fn smart_resize_up(ctx) { if is_vim_pane(ctx) { action.send_keys_current("<M-k>") } else { action.resize_up(3) } }
fn smart_resize_right(ctx) { if is_vim_pane(ctx) { action.send_keys_current("<M-l>") } else { action.resize_right(3) } }

// C-\ goes back to the previously focused pane (select-pane -l).
fn smart_last_pane(ctx) { if is_vim_pane(ctx) { action.send_keys_current("<C-\\>") } else { action.last_pane() } }

define_action("smart-nav-left", smart_nav_left);
define_action("smart-nav-down", smart_nav_down);
define_action("smart-nav-up", smart_nav_up);
define_action("smart-nav-right", smart_nav_right);
define_action("smart-resize-left", smart_resize_left);
define_action("smart-resize-down", smart_resize_down);
define_action("smart-resize-up", smart_resize_up);
define_action("smart-resize-right", smart_resize_right);
define_action("smart-last-pane", smart_last_pane);

bind("normal", "<C-h>", "smart-nav-left");
bind("normal", "<C-j>", "smart-nav-down");
bind("normal", "<C-k>", "smart-nav-up");
bind("normal", "<C-l>", "smart-nav-right");
bind("normal", "<M-h>", "smart-resize-left");
bind("normal", "<M-j>", "smart-resize-down");
bind("normal", "<M-k>", "smart-resize-up");
bind("normal", "<M-l>", "smart-resize-right");
bind("normal", "<C-\\>", "smart-last-pane");

// copy-mode-vi: copy mode never runs over Neovim's full screen, so the same
// keys just move between panes.
for mode in ["copy", "select"] {
    bind(mode, "<C-h>", action.focus_left());
    bind(mode, "<C-j>", action.focus_down());
    bind(mode, "<C-k>", action.focus_up());
    bind(mode, "<C-l>", action.focus_right());
}

// ---------------------------------------------------------------------------
// Theme and tab bar
// ---------------------------------------------------------------------------
// Catppuccin Frappe with rounded window tabs (the Powerline glyphs U+E0B6 and
// U+E0B4, which need a Nerd Font). Each window is labelled from
// the pane it shows: "nvim <dir>" for Neovim, "$ <dir>" for a shell, else the
// pane title or program, cut to 24 characters. The right side shows the
// focused pane's directory.
theme.set_palette(#{
    base: "#303446",
    surface: "#51576d",
    text: "#c6d0f5",
    accent: "#ca9ee6",
    urgent: "#e78284"
});

// pane-active-border-style fg=magenta, pane-border-style fg=brightblack: the
// dividers next to the focused pane in magenta, the others in bright black.
theme.set_pane_active_border_color("magenta");
theme.set_pane_border_color("brightblack");

// popup-border-lines rounded, with the focused popup's frame in the accent
// color and the others in Frappe's overlay 0.
theme.set_popup_border_lines("rounded");
theme.set_popup_active_border_color("#ca9ee6");
theme.set_popup_border_color("#737994");

// The last two components of a path: /home/me/src/app -> src/app.
fn short_path(path) {
    if path == () {
        return "";
    }
    let parts = [];
    for part in path.split("/") {
        if part != "" {
            parts.push(part);
        }
    }
    if parts.len() > 2 {
        parts = parts.extract(parts.len() - 2);
    }
    let short = "";
    for part in parts {
        short += if short == "" { part } else { "/" + part };
    }
    if path.starts_with("/") && parts.len() < 2 { "/" + short } else { short }
}

fn is_shell(name) {
    ["sh", "bash", "zsh", "fish", "nu", "dash", "ksh"].contains(name)
}

fn window_label(tab) {
    let program = tab.process_name();
    let label = if program == "nvim" {
        "nvim " + short_path(tab.cwd())
    } else if program != () && is_shell(program) {
        "$ " + short_path(tab.cwd())
    } else if tab.pane_title() != () && tab.pane_title() != "" {
        tab.pane_title()
    } else if program != () {
        program
    } else {
        tab.title()
    };
    if label.len() > 24 {
        label.sub_string(0, 23) + "…"
    } else {
        label
    }
}

fn format_tabs(ctx) {
    let left = [];
    for tab in ctx.tabs() {
        let bg = if tab.is_active() { theme.color("accent") } else { theme.color("surface") };
        let fg = if tab.is_active() { theme.color("base") } else { theme.color("text") };
        let label = if ctx.is_root() { `${tab.number()} ${window_label(tab)}` } else { tab.title() };
        left.push(ui.segment("\uE0B6", #{ fg: bg }));
        left.push(ui.segment(label, #{ fg: fg, bg: bg, bold: tab.is_active() }));
        left.push(ui.segment("\uE0B4", #{ fg: bg }));
        if tab.has_bell() {
            left.push(ui.segment(" !", #{ fg: theme.color("urgent") }));
        }
        if tab.synchronize_panes() {
            left.push(ui.segment(" sync", #{ fg: theme.color("urgent") }));
        }
        left.push(ui.segment(" "));
    }

    let right = [];
    if ctx.is_root() {
        let active = ctx.tabs()[ctx.active_index()];
        if active.cwd() != () {
            right.push(ui.segment(" " + active.cwd() + " ", #{ fg: theme.color("text") }));
        }
    }
    ui.bar(left, [], right)
}

tabbar.set_formatter(format_tabs);
tabbar.set_position("bottom");   // status-position bottom
