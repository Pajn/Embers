use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use embers_client::scripting::{build_mdbook, generate_config_api_docs};
use tempfile::tempdir;

#[test]
fn generated_docs_cover_representative_exports() -> Result<(), Box<dyn std::error::Error>> {
    let tempdir = tempdir().unwrap();
    generate_config_api_docs(tempdir.path())?;

    let action = fs::read_to_string(tempdir.path().join("action.md"))?;
    let registration_action = fs::read_to_string(tempdir.path().join("registration-action.md"))?;
    let tree = fs::read_to_string(tempdir.path().join("tree.md"))?;
    let registration_tree = fs::read_to_string(tempdir.path().join("registration-tree.md"))?;
    let context = fs::read_to_string(tempdir.path().join("context.md"))?;
    let buffer_ref = fs::read_to_string(tempdir.path().join("buffer-ref.md"))?;
    let mux = fs::read_to_string(tempdir.path().join("mux.md"))?;
    let registration_system = fs::read_to_string(tempdir.path().join("registration-system.md"))?;
    let registration_ui = fs::read_to_string(tempdir.path().join("registration-ui.md"))?;
    let book_toml = fs::read_to_string(tempdir.path().join("book.toml"))?;
    let tabs_js = fs::read_to_string(tempdir.path().join("theme/rhai-autodocs-tabs.js"))?;
    let rhai_highlight = fs::read_to_string(tempdir.path().join("theme/rhai-highlight.js"))?;
    let registration_defs = fs::read_to_string(tempdir.path().join("defs/registration.rhai"))?;
    let runtime_defs = fs::read_to_string(tempdir.path().join("defs/runtime.rhai"))?;

    assert!(action.contains("focus_left"));
    assert!(registration_action.contains("focus_left"));
    assert!(tree.contains("buffer_spawn"));
    assert!(registration_tree.contains("buffer_spawn"));
    assert!(context.contains("current_buffer"));
    assert!(buffer_ref.contains("history_text"));
    assert!(mux.contains("current_session"));
    assert!(registration_system.contains("env"));
    assert!(registration_ui.contains("segment"));
    assert!(registration_defs.contains("fn bind("));
    assert!(registration_defs.contains("let action: ActionApi;"));
    assert!(registration_defs.contains("let tree: TreeApi;"));
    assert!(registration_defs.contains("let ui: UiApi;"));
    assert!(registration_defs.contains("let system: SystemApi;"));
    assert!(!registration_defs.contains("ScriptResult"));
    assert!(!registration_defs.contains("Result<"));
    assert!(!registration_defs.contains("EvalAltResult"));
    assert!(runtime_defs.contains("let action: ActionApi;"));
    assert!(runtime_defs.contains("fn noop(_: ActionApi) -> Action;"));
    assert!(runtime_defs.contains("fn enter_mode(_: ActionApi, mode: string) -> Action;"));
    assert!(!runtime_defs.contains("Result<"));
    assert!(!runtime_defs.contains("EvalAltResult"));
    assert!(
        runtime_defs
            .contains("fn send_keys(_: ActionApi, buffer_id: int, notation: string) -> Action;")
    );
    assert!(runtime_defs.contains("fn buffer_spawn(_: TreeApi, command: array) -> TreeSpec;"));
    assert!(runtime_defs.contains(
        "fn split(_: TreeApi, direction: string, children: array, sizes: array) -> TreeSpec;"
    ));
    assert!(runtime_defs.contains("fn mode(bar: TabBarContext) ->"));
    assert!(runtime_defs.contains("fn index(tab: TabInfo) ->"));
    assert!(runtime_defs.contains("fn current_session(mux: MuxApi) ->"));
    assert!(buffer_ref.contains("ReturnType: `string | ()`"));
    assert!(mux.contains("ReturnType: `SessionRef | ()`"));
    assert!(book_toml.contains("theme/rhai-autodocs-tabs.js"));
    assert!(book_toml.contains("theme/rhai-highlight.js"));
    assert!(tabs_js.contains("window.openTab"));
    assert!(rhai_highlight.contains("registerLanguage(\"rhai\""));
    Ok(())
}

#[test]
fn checked_in_docs_are_current() -> Result<(), Box<dyn std::error::Error>> {
    let tempdir = tempdir().unwrap();
    generate_config_api_docs(tempdir.path())?;
    build_mdbook(tempdir.path())?;

    let generated = read_tree_bytes(tempdir.path())?;
    let checked_in = read_tree_bytes(&repo_docs_dir())?;

    if generated != checked_in {
        panic!(
            "checked-in config docs are stale:\n{}",
            summarize_doc_tree_diff(&generated, &checked_in)
        );
    }

    let generated_book = read_tree_bytes(&generated_book_dir(tempdir.path()))?;
    let checked_in_book = read_tree_bytes(&repo_docs_book_dir())?;

    if generated_book != checked_in_book {
        panic!(
            "checked-in config book is stale:\n{}",
            summarize_doc_tree_diff(&generated_book, &checked_in_book)
        );
    }
    Ok(())
}

fn repo_docs_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/config-api")
}

fn repo_docs_book_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/config-api-book")
}

fn generated_book_dir(output_dir: &Path) -> PathBuf {
    output_dir.with_file_name(format!(
        "{}-book",
        output_dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("config-api")
    ))
}

fn read_tree_bytes(root: &Path) -> std::io::Result<BTreeMap<String, Vec<u8>>> {
    let mut entries = BTreeMap::new();
    visit_bytes(root, root, &mut entries)?;
    Ok(entries)
}

fn visit_bytes(
    root: &Path,
    path: &Path,
    entries: &mut BTreeMap<String, Vec<u8>>,
) -> std::io::Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            visit_bytes(root, &path, entries)?;
            continue;
        }
        let relative = path.strip_prefix(root).map_err(|error| {
            std::io::Error::other(format!(
                "failed to strip root {} from {}: {error}",
                root.display(),
                path.display()
            ))
        })?;
        let relative = relative.to_string_lossy().replace('\\', "/");
        entries.insert(relative, fs::read(&path)?);
    }
    Ok(())
}

fn summarize_doc_tree_diff(
    generated: &BTreeMap<String, Vec<u8>>,
    checked_in: &BTreeMap<String, Vec<u8>>,
) -> String {
    let mut summary = Vec::new();

    for path in generated.keys() {
        if !checked_in.contains_key(path) {
            summary.push(format!("extra generated file: {path}"));
        }
    }

    for path in checked_in.keys() {
        if !generated.contains_key(path) {
            summary.push(format!("missing generated file: {path}"));
        }
    }

    for (path, generated_content) in generated {
        let Some(checked_in_content) = checked_in.get(path) else {
            continue;
        };
        if generated_content == checked_in_content {
            continue;
        }
        let detail = first_difference(generated_content, checked_in_content)
            .unwrap_or_else(|| "content differs".to_owned());
        summary.push(format!("changed file: {path} ({detail})"));
    }

    if summary.is_empty() {
        "content differs, but no specific file-level summary was produced".to_owned()
    } else {
        summary.join("\n")
    }
}

fn first_difference(generated: &[u8], checked_in: &[u8]) -> Option<String> {
    match (
        std::str::from_utf8(generated),
        std::str::from_utf8(checked_in),
    ) {
        (Ok(generated), Ok(checked_in)) => {
            for (index, (generated_line, checked_in_line)) in
                generated.lines().zip(checked_in.lines()).enumerate()
            {
                if generated_line != checked_in_line {
                    let line = index + 1;
                    return Some(format!(
                        "first differing line {line}: generated=`{}` checked_in=`{}`",
                        generated_line, checked_in_line
                    ));
                }
            }

            let generated_lines = generated.lines().count();
            let checked_in_lines = checked_in.lines().count();
            (generated_lines != checked_in_lines).then(|| {
                format!(
                    "line count differs: generated={generated_lines} checked_in={checked_in_lines}"
                )
            })
        }
        _ => Some(format!(
            "binary content differs: generated={} bytes checked_in={} bytes",
            generated.len(),
            checked_in.len()
        )),
    }
}
