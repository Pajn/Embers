use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::path::Path;
use std::process::Command;

use rhai::Engine;
use rhai_autodocs::{export, generate, item::Item, module::Documentation};

use super::context::Context;
use super::engine::{documentation_registration_scope, register_documented_registration_api};
use super::runtime::{
    register_documented_registration_runtime_api, register_documented_runtime_api, runtime_scope,
};
use super::types::ThemeSpec;

struct PageSpec<'a> {
    file: &'a str,
    title: &'a str,
    intro: &'a str,
    receiver: Option<&'a str>,
    names: &'a [&'a str],
}

const REGISTRATION_PAGES: &[PageSpec<'_>] = &[
    PageSpec {
        file: "registration-globals.md",
        title: "Registration Globals",
        intro: "Top-level functions available while the config file is being evaluated.",
        receiver: None,
        names: &[
            "set_leader",
            "define_mode",
            "bind",
            "unbind",
            "define_action",
            "on",
        ],
    },
    PageSpec {
        file: "registration-action.md",
        title: "Action (Registration)",
        intro: "Action builders available through the top-level `action` object during config evaluation and also inside runtime callbacks. Registration-time docs include the full builder surface used while declaring bindings and named actions.",
        receiver: Some("ActionApi"),
        names: &[],
    },
    PageSpec {
        file: "registration-tree.md",
        title: "Tree (Registration)",
        intro: "Tree builders available through the top-level `tree` object during config evaluation and also inside runtime callbacks.",
        receiver: Some("TreeApi"),
        names: &[],
    },
    PageSpec {
        file: "registration-system.md",
        title: "System (Registration)",
        intro: "System helpers available through the top-level `system` object during config evaluation and also inside runtime callbacks.",
        receiver: Some("SystemApi"),
        names: &[],
    },
    PageSpec {
        file: "registration-ui.md",
        title: "UI (Registration)",
        intro: "UI helpers available through the top-level `ui` object during config evaluation and also inside runtime callbacks.",
        receiver: Some("UiApi"),
        names: &[],
    },
    PageSpec {
        file: "mouse.md",
        title: "Mouse",
        intro: "Mouse registration methods available through the `mouse` config object.",
        receiver: Some("MouseApi"),
        names: &[],
    },
    PageSpec {
        file: "theme.md",
        title: "Theme",
        intro: "Theme registration methods available through the `theme` config object.",
        receiver: Some("ThemeApi"),
        names: &[],
    },
    PageSpec {
        file: "tabbar.md",
        title: "Tabbar",
        intro: "Tab bar registration methods available through the `tabbar` config object.",
        receiver: Some("TabbarApi"),
        names: &[],
    },
];

const RUNTIME_PAGES: &[PageSpec<'_>] = &[
    PageSpec {
        file: "action.md",
        title: "Action",
        intro: "Action builders available through the runtime `action` object inside named actions and event handlers. This runtime page shows only the builders supported by the live executor.",
        receiver: Some("ActionApi"),
        names: &[],
    },
    PageSpec {
        file: "tree.md",
        title: "Tree",
        intro: "Tree builders available through the runtime `tree` object. The same helper surface is also available through the top-level `tree` object during config evaluation.",
        receiver: Some("TreeApi"),
        names: &[],
    },
    PageSpec {
        file: "context.md",
        title: "Context",
        intro: "State inspection helpers available on the action/event callback context argument.",
        receiver: Some("Context"),
        names: &[],
    },
    PageSpec {
        file: "mux.md",
        title: "Mux",
        intro: "State inspection helpers available through the runtime `mux` object when a live callback has mux context.",
        receiver: Some("MuxApi"),
        names: &[],
    },
    PageSpec {
        file: "event-info.md",
        title: "EventInfo",
        intro: "Event metadata available when a callback is triggered from an emitted event.",
        receiver: Some("EventInfo"),
        names: &[],
    },
    PageSpec {
        file: "session-ref.md",
        title: "SessionRef",
        intro: "Session reference helpers returned from context queries.",
        receiver: Some("SessionRef"),
        names: &[],
    },
    PageSpec {
        file: "buffer-ref.md",
        title: "BufferRef",
        intro: "Buffer inspection helpers returned from context queries.",
        receiver: Some("BufferRef"),
        names: &[],
    },
    PageSpec {
        file: "node-ref.md",
        title: "NodeRef",
        intro: "Node inspection helpers returned from context queries.",
        receiver: Some("NodeRef"),
        names: &[],
    },
    PageSpec {
        file: "floating-ref.md",
        title: "FloatingRef",
        intro: "Floating window inspection helpers returned from context queries.",
        receiver: Some("FloatingRef"),
        names: &[],
    },
    PageSpec {
        file: "tab-bar-context.md",
        title: "TabBarContext",
        intro: "Formatter helpers passed to the tab bar formatter callback.",
        receiver: Some("TabBarContext"),
        names: &[],
    },
    PageSpec {
        file: "tab-info.md",
        title: "TabInfo",
        intro: "Per-tab metadata used by the tab bar formatter.",
        receiver: Some("TabInfo"),
        names: &[],
    },
    PageSpec {
        file: "system-runtime.md",
        title: "System",
        intro: "System helpers available through the runtime `system` object. The same helper surface is also available through the top-level `system` object during config evaluation.",
        receiver: Some("SystemApi"),
        names: &[],
    },
    PageSpec {
        file: "ui.md",
        title: "UI",
        intro: "Tab bar rendering helpers available through the runtime `ui` object. The same helper surface is also available through the top-level `ui` object during config evaluation.",
        receiver: Some("UiApi"),
        names: &[],
    },
    PageSpec {
        file: "runtime-theme.md",
        title: "Runtime Theme",
        intro: "Palette lookup helpers available through the runtime `theme` object in formatter callbacks.",
        receiver: Some("ThemeRuntimeApi"),
        names: &[],
    },
];

pub fn generate_config_api_docs(output_dir: &Path) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(output_dir)?;
    let defs_dir = output_dir.join("defs");
    fs::create_dir_all(&defs_dir)?;
    let theme_dir = output_dir.join("theme");
    fs::create_dir_all(&theme_dir)?;
    let registration_defs_path = defs_dir.join("registration.rhai");
    let runtime_defs_path = defs_dir.join("runtime.rhai");

    let registration_engine = build_registration_docs_engine();
    let registration_scope = documentation_registration_scope();
    registration_engine
        .definitions_with_scope(&registration_scope)
        .include_standard_packages(false)
        .write_to_file(&registration_defs_path)?;

    let runtime_engine = build_runtime_docs_engine();
    let runtime_scope = runtime_scope(Some(Context::default()), ThemeSpec::default());
    runtime_engine
        .definitions_with_scope(&runtime_scope)
        .include_standard_packages(false)
        .write_to_file(&runtime_defs_path)?;

    let registration_docs = export::options()
        .include_standard_packages(false)
        .export(&registration_engine)?;
    let runtime_docs = export::options()
        .include_standard_packages(false)
        .export(&runtime_engine)?;

    write_page_set(output_dir, &registration_docs, REGISTRATION_PAGES)?;
    write_page_set(output_dir, &runtime_docs, RUNTIME_PAGES)?;
    fs::write(output_dir.join("index.md"), index_page())?;
    fs::write(output_dir.join("example.md"), example_page())?;
    fs::write(output_dir.join("SUMMARY.md"), summary_page())?;
    fs::write(output_dir.join("book.toml"), book_toml())?;
    fs::write(
        theme_dir.join("rhai-autodocs-tabs.js"),
        rhai_autodocs_tabs_js(),
    )?;
    fs::write(theme_dir.join("rhai-highlight.js"), rhai_highlight_js())?;

    Ok(())
}

fn build_registration_docs_engine() -> Engine {
    let mut engine = Engine::new();
    engine.set_max_expr_depths(256, 256);
    engine.set_max_operations(1_000_000);
    register_documented_registration_api(&mut engine);
    register_documented_registration_runtime_api(&mut engine);
    engine
}

fn build_runtime_docs_engine() -> Engine {
    let mut engine = Engine::new();
    engine.set_max_expr_depths(256, 256);
    engine.set_max_operations(1_000_000);
    register_documented_runtime_api(&mut engine);
    engine
}

fn write_page_set(
    output_dir: &Path,
    docs: &Documentation,
    pages: &[PageSpec<'_>],
) -> Result<(), Box<dyn Error>> {
    for page in pages {
        let items = docs
            .items
            .iter()
            .filter_map(|item| filter_item_for_page(item, page))
            .collect::<Vec<_>>();
        if items.is_empty() {
            return Err(format!(
                "page '{}' ({}) matched no exported items",
                page.title, page.file
            )
            .into());
        }

        let page_doc = Documentation {
            namespace: docs.namespace.clone(),
            name: page.title.to_owned(),
            sub_modules: Vec::new(),
            documentation: page.intro.to_owned(),
            items,
        };
        let rendered = generate::mdbook().generate(&page_doc)?;
        let content = rendered
            .get(page.title)
            .cloned()
            .ok_or_else(|| format!("missing rendered page for {}", page.title))?;
        fs::write(output_dir.join(page.file), content)?;
    }
    Ok(())
}

fn filter_item_for_page(item: &Item, page: &PageSpec<'_>) -> Option<Item> {
    if let Some(receiver) = page.receiver {
        let Item::Function {
            root_metadata: _,
            metadata,
            name,
            index,
        } = item
        else {
            return None;
        };

        let filtered = metadata
            .iter()
            .filter(|metadata| {
                metadata
                    .params
                    .as_ref()
                    .and_then(|params| params.first())
                    .and_then(|param| param.get("type"))
                    .map(String::as_str)
                    .map(normalize_type_name)
                    .is_some_and(|ty| ty == receiver)
            })
            .cloned()
            .collect::<Vec<_>>();
        if filtered.is_empty() {
            return None;
        }
        let mut filtered = filtered;
        filtered.sort_by(|left, right| left.signature.cmp(&right.signature));
        let root_metadata = filtered
            .iter()
            .find(|metadata| metadata.doc_comments.is_some())
            .cloned()
            .unwrap_or_else(|| filtered[0].clone());

        Some(Item::Function {
            root_metadata,
            metadata: filtered,
            name: name.clone(),
            index: *index,
        })
    } else {
        if !page.names.iter().any(|name| item_name(item) == *name) {
            return None;
        }
        match item {
            Item::Function {
                metadata,
                name,
                index,
                ..
            } => {
                let mut metadata = metadata.clone();
                metadata.sort_by(|left, right| left.signature.cmp(&right.signature));
                let root_metadata = metadata
                    .iter()
                    .find(|metadata| metadata.doc_comments.is_some())
                    .cloned()
                    .unwrap_or_else(|| metadata[0].clone());
                Some(Item::Function {
                    root_metadata,
                    metadata,
                    name: name.clone(),
                    index: *index,
                })
            }
            Item::CustomType { .. } => Some(item.clone()),
        }
    }
}

fn item_name(item: &Item) -> &str {
    match item {
        Item::Function { name, .. } => name,
        Item::CustomType { metadata, .. } => &metadata.display_name,
    }
}

fn normalize_type_name(ty: &str) -> &str {
    ty.trim_start_matches("&mut ")
        .rsplit("::")
        .next()
        .unwrap_or(ty)
}

fn index_page() -> String {
    let mut files = BTreeSet::new();
    files.extend(REGISTRATION_PAGES.iter().map(|page| page.file));
    files.extend(RUNTIME_PAGES.iter().map(|page| page.file));
    let links = files
        .into_iter()
        .map(|file| format!("- [{}]({file})", file.trim_end_matches(".md")))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "# Embers Config API\n\n\
This reference is generated from the Rust-backed Rhai exports used by Embers.\n\n\
There are two execution phases:\n\n\
- registration time: the top-level config file where you declare modes, bindings, named actions, and visual settings\n\
- runtime: named actions, event handlers, and tab bar formatters that run against live client state\n\n\
Definition files live in [`defs/`](defs/).\n\n\
## Pages\n\n\
{links}\n\n\
## Definitions\n\n\
- [`registration.rhai`](defs/registration.rhai)\n\
- [`runtime.rhai`](defs/runtime.rhai)\n\n\
## Example\n\n\
- [`example.md`](example.md)\n"
    )
}

fn summary_page() -> String {
    let registration_links = REGISTRATION_PAGES
        .iter()
        .map(|page| format!("- [{}]({})", page.title, page.file))
        .collect::<Vec<_>>()
        .join("\n");
    let runtime_links = RUNTIME_PAGES
        .iter()
        .map(|page| format!("- [{}]({})", page.title, page.file))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "# Summary\n\n\
- [Overview](index.md)\n\
- [Example](example.md)\n\
{registration_links}\n\
{runtime_links}\n"
    )
}

fn book_toml() -> &'static str {
    r#"[book]
title = "Embers Config API"
language = "en"
src = "."

[output.html]
default-theme = "light"
additional-js = ["theme/rhai-autodocs-tabs.js", "theme/rhai-highlight.js"]
"#
}

fn rhai_autodocs_tabs_js() -> &'static str {
    r#"window.openTab = function (evt, group, tab) {
  document
    .querySelectorAll('.tabcontent[group="' + group + '"]')
    .forEach(function (content) {
      content.style.display = "none";
    });

  document
    .querySelectorAll('.tablinks[group="' + group + '"]')
    .forEach(function (link) {
      link.classList.remove("active");
    });

  const target = document.getElementById(group + "-" + tab);
  if (target) {
    target.style.display = "block";
  }

  if (evt && evt.currentTarget) {
    evt.currentTarget.classList.add("active");
  }
};"#
}

fn rhai_highlight_js() -> &'static str {
    r#"(function () {
  const hljsInstance = window.hljs;
  if (!hljsInstance) {
    return;
  }

  hljsInstance.registerLanguage("rhai", function (hljs) {
    return {
      name: "Rhai",
      aliases: ["rhai-script"],
      keywords: {
        keyword:
          "if else switch do while loop for in break continue return throw try catch fn private let const import export as and or not",
        literal: "true false null"
      },
      contains: [
        hljs.C_LINE_COMMENT_MODE,
        hljs.C_BLOCK_COMMENT_MODE,
        hljs.APOS_STRING_MODE,
        hljs.QUOTE_STRING_MODE,
        hljs.C_NUMBER_MODE,
        {
          className: "literal",
          begin: /#\{/,
          end: /\}/
        },
        {
          className: "function",
          beginKeywords: "fn",
          end: /[{;]/,
          excludeEnd: true,
          contains: [
            hljs.UNDERSCORE_TITLE_MODE,
            {
              className: "params",
              begin: /\(/,
              end: /\)/,
              contains: [
                hljs.C_LINE_COMMENT_MODE,
                hljs.C_BLOCK_COMMENT_MODE,
                hljs.APOS_STRING_MODE,
                hljs.QUOTE_STRING_MODE,
                hljs.C_NUMBER_MODE
              ]
            }
          ]
        }
      ]
    };
  });

  const highlightRhaiBlocks = function () {
    document
      .querySelectorAll("pre code.language-rhai, pre code.lang-rhai")
      .forEach(function (block) {
        if (typeof hljsInstance.highlightElement === "function") {
          block.removeAttribute("data-highlighted");
          hljsInstance.highlightElement(block);
          return;
        }

        if (typeof hljsInstance.highlightBlock === "function") {
          hljsInstance.highlightBlock(block);
        }
      });
  };

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", highlightRhaiBlocks);
  } else {
    highlightRhaiBlocks();
  }
})();"#
}

pub fn build_mdbook(output_dir: &Path) -> Result<(), Box<dyn Error>> {
    let build_dir = output_dir.with_file_name(format!(
        "{}-book",
        output_dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("config-api")
    ));

    let status = Command::new("mdbook")
        .arg("build")
        .arg(output_dir)
        .arg("--dest-dir")
        .arg(&build_dir)
        .status()?;

    if !status.success() {
        return Err(format!("mdbook build failed for {}", output_dir.display()).into());
    }

    Ok(())
}

fn example_page() -> &'static str {
    r##"# Example

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
"##
}
