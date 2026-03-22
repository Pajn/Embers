// Embers config derived from tmux settings in:
// /Users/emma/Development/nix-config/home/base/shell/default.nix
//
// Notes:
// - This mirrors the tmux leader, split/navigation keys, popup helpers, bell
//   handling, history helpers, and a Catppuccin Frappe-inspired tab bar.
// - History helpers use Embers' buffer snapshot/history APIs rather than tmux's
//   capture-pane commands.
// - Tab bars are node-scoped, so one formatter is shared by root and nested tabs.

set_leader("<C-a>");

fn current_cwd(ctx) {
    let buffer = ctx.current_buffer();
    if buffer == () {
        ()
    } else {
        buffer.cwd()
    }
}

fn shell_tree(ctx, tree_api, title) {
    tree_api.buffer_spawn(
        ["/bin/zsh"],
        #{
            title: title,
            cwd: current_cwd(ctx)
        }
    )
}

fn command_tree(ctx, tree_api, title, command) {
    tree_api.buffer_spawn(
        command,
        #{
            title: title,
            cwd: current_cwd(ctx)
        }
    )
}

fn history_tree(ctx, tree_api, title, visible_only) {
    let buffer = ctx.current_buffer();
    if buffer == () {
        return shell_tree(ctx, tree_api, title);
    }

    let history = if visible_only {
        buffer.snapshot_text(200)
    } else {
        buffer.history_text()
    };

    tree_api.buffer_spawn(
        ["/bin/sh", "-lc", "printf '%s' \"$EMBERS_HISTORY\" | less -R"],
        #{
            title: title,
            cwd: current_cwd(ctx),
            env: #{
                EMBERS_HISTORY: history
            }
        }
    )
}

fn popup(action_api, tree_spec, title, x, y, width, height) {
    action_api.open_floating(
        tree_spec,
        #{
            title: title,
            x: x,
            y: y,
            width: width,
            height: height
        }
    )
}

fn is_nvim_buffer(ctx) {
    let buffer = ctx.current_buffer();
    let node = ctx.current_node();
    if buffer == () || node == () {
        return false;
    }

    let name = buffer.process_name();
    if name == () {
        return false;
    }

    buffer.is_visible()
        && !buffer.is_detached()
        && node.kind() == "buffer_view"
        && (name == "nvim" || name == "vim" || name == "nvr")
}

fn smart_nav_left(ctx) {
    if is_nvim_buffer(ctx) {
        action.send_bytes_current([8])
    } else {
        action.focus_left()
    }
}

fn smart_nav_down(ctx) {
    if is_nvim_buffer(ctx) {
        action.send_bytes_current([10])
    } else {
        action.focus_down()
    }
}

fn smart_nav_up(ctx) {
    if is_nvim_buffer(ctx) {
        action.send_bytes_current([11])
    } else {
        action.focus_up()
    }
}

fn smart_nav_right(ctx) {
    if is_nvim_buffer(ctx) {
        action.send_bytes_current([12])
    } else {
        action.focus_right()
    }
}

fn smart_resize_left(ctx) {
    if is_nvim_buffer(ctx) {
        action.send_bytes_current([27, 104])
    } else {
        action.resize_left(3)
    }
}

fn smart_resize_down(ctx) {
    if is_nvim_buffer(ctx) {
        action.send_bytes_current([27, 106])
    } else {
        action.resize_down(3)
    }
}

fn smart_resize_up(ctx) {
    if is_nvim_buffer(ctx) {
        action.send_bytes_current([27, 107])
    } else {
        action.resize_up(3)
    }
}

fn smart_resize_right(ctx) {
    if is_nvim_buffer(ctx) {
        action.send_bytes_current([27, 108])
    } else {
        action.resize_right(3)
    }
}

fn split_below(ctx) {
    // tmux split-window -v creates a top/bottom split.
    action.split_with("horizontal", shell_tree(ctx, tree, "shell"))
}

fn split_right(ctx) {
    // tmux split-window -h creates a left/right split.
    action.split_with("vertical", shell_tree(ctx, tree, "shell"))
}

fn new_shell_tab(ctx) {
    action.insert_tab_after_current("shell", shell_tree(ctx, tree, "shell"))
}

fn new_yazi_tab(ctx) {
    action.insert_tab_after_current(
        "yazi",
        command_tree(ctx, tree, "yazi", ["/usr/bin/env", "yazi"])
    )
}

fn shell_popup(ctx) {
    popup(action, shell_tree(ctx, tree, "shell"), "shell", 8, 2, 100, 28)
}

fn lazygit_popup(ctx) {
    popup(
        action,
        command_tree(ctx, tree, "lazygit", ["/usr/bin/env", "lazygit"]),
        "lazygit",
        4,
        1,
        110,
        32
    )
}

fn git_log_popup(ctx) {
    popup(
        action,
        command_tree(ctx, tree, "git-log", ["/bin/sh", "-lc", "git log | nvim -R -"]),
        "git log",
        4,
        1,
        110,
        32
    )
}

fn visible_history_tab(ctx) {
    action.insert_tab_after_current(
        "visible-history",
        history_tree(ctx, tree, "visible-history", true)
    )
}

fn full_history_tab(ctx) {
    action.insert_tab_after_current(
        "full-history",
        history_tree(ctx, tree, "full-history", false)
    )
}

fn scratchpad(ctx) {
    popup(
        action,
        tree.tabs_with_active([
            tree.tab("shell", shell_tree(ctx, tree, "scratch")),
            tree.tab(
                "tools",
                tree.split_v([
                    command_tree(ctx, tree, "yazi", ["/usr/bin/env", "yazi"]),
                    shell_tree(ctx, tree, "tools")
                ])
            )
        ], 0),
        "scratchpad",
        10,
        3,
        100,
        28
    )
}

fn format_tabs(ctx) {
    let left = [];
    let active_bg = theme.color("active_bg");
    let active_fg = theme.color("active_fg");
    let inactive_bg = theme.color("inactive_bg");
    let inactive_fg = theme.color("inactive_fg");
    let urgent_fg = theme.color("urgent_fg");

    for tab in ctx.tabs() {
        let title = " " + tab.title() + " ";
        if tab.is_active() {
            left.push(ui.segment("", #{ fg: active_bg }));
            left.push(ui.segment(title, #{
                fg: active_fg,
                bg: active_bg
            }));
            left.push(ui.segment("", #{ fg: active_bg }));
        } else {
            left.push(ui.segment("", #{ fg: inactive_bg }));
            left.push(ui.segment(title, #{
                fg: inactive_fg,
                bg: inactive_bg
            }));
            left.push(ui.segment("", #{ fg: inactive_bg }));
        }

        if tab.has_urgent() {
            left.push(ui.segment(" ! ", #{ fg: urgent_fg }));
        }

        left.push(ui.segment(" "));
    }

    ui.bar(left, [], [])
}

fn bell_handler(ctx) {
    let event = ctx.event();
    if event == () || event.name() != "buffer_bell" {
        return;
    }

    let buffer_id = event.buffer_id();
    if buffer_id == () {
        return;
    }

    let buffer = ctx.find_buffer(buffer_id);
    if buffer == () || buffer.is_visible() {
        return;
    }

    action.move_buffer_to_floating(
        buffer.id(),
        #{
            title: buffer.title(),
            x: 4,
            y: 1,
            width: 110,
            height: 32,
            focus: true
        }
    )
}

theme.set_palette(#{
    active_bg: "#ca9ee6",
    active_fg: "#303446",
    inactive_bg: "#51576d",
    inactive_fg: "#c6d0f5",
    urgent_fg: "#e78284"
});

tabbar.set_formatter(format_tabs);

define_action("smart-nav-left", smart_nav_left);
define_action("smart-nav-down", smart_nav_down);
define_action("smart-nav-up", smart_nav_up);
define_action("smart-nav-right", smart_nav_right);
define_action("smart-resize-left", smart_resize_left);
define_action("smart-resize-down", smart_resize_down);
define_action("smart-resize-up", smart_resize_up);
define_action("smart-resize-right", smart_resize_right);
define_action("split-below", split_below);
define_action("split-right", split_right);
define_action("new-shell-tab", new_shell_tab);
define_action("new-yazi-tab", new_yazi_tab);
define_action("shell-popup", shell_popup);
define_action("lazygit-popup", lazygit_popup);
define_action("git-log-popup", git_log_popup);
define_action("visible-history-tab", visible_history_tab);
define_action("full-history-tab", full_history_tab);
define_action("scratchpad", scratchpad);

bind("normal", "<C-h>", "smart-nav-left");
bind("normal", "<C-j>", "smart-nav-down");
bind("normal", "<C-k>", "smart-nav-up");
bind("normal", "<C-l>", "smart-nav-right");
bind("normal", "<M-h>", "smart-resize-left");
bind("normal", "<M-j>", "smart-resize-down");
bind("normal", "<M-k>", "smart-resize-up");
bind("normal", "<M-l>", "smart-resize-right");
bind("copy", "<C-h>", "smart-nav-left");
bind("copy", "<C-j>", "smart-nav-down");
bind("copy", "<C-k>", "smart-nav-up");
bind("copy", "<C-l>", "smart-nav-right");
bind("normal", "<leader>c", "new-shell-tab");
bind("normal", "<leader>s", "split-below");
bind("normal", "<leader>v", "split-right");
bind("normal", "<leader>d", "shell-popup");
bind("normal", "<leader>g", "lazygit-popup");
bind("normal", "<leader>G", "git-log-popup");
bind("normal", "<leader>h", "visible-history-tab");
bind("normal", "<leader>H", "full-history-tab");
bind("normal", "<leader>o", "scratchpad");

on("buffer_bell", bell_handler);
