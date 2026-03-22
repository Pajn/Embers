# Example

This is a trimmed example based on the repository fixture config. It shows the two main phases together.

```rhai
set_leader("<C-a>");

fn shell_tree(ctx) {
    tree.buffer_spawn(
        ["/bin/zsh"],
        #{
            title: "shell",
            cwd: if ctx.current_buffer() == () { () } else { ctx.current_buffer().cwd() }
        }
    )
}

fn split_below(ctx) {
    action.split_with("horizontal", shell_tree(ctx))
}

fn format_tabs(ctx) {
    let active = ctx.tabs()[ctx.active_index()];
    ui.bar([
        ui.segment(" " + active.title() + " ", #{
            fg: theme.color("active_fg"),
            bg: theme.color("active_bg")
        })
    ], [], [])
}

define_action("split-below", split_below);
bind("normal", "<leader>\"", "split-below");
theme.set_palette(#{
    active_fg: "#303446",
    active_bg: "#c6d0f5"
});
tabbar.set_formatter(format_tabs);
mouse.set_click_focus(true);
```
