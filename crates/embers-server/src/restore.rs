//! Restoring panes whose runtime keeper is gone, for example after a reboot.
//!
//! Such a pane cannot be reattached, so the server starts a fresh process for it
//! in its last known working directory. Whether the program it last ran is
//! started again is decided by the [`RestoreConfig`] allowlist, like
//! tmux-resurrect: a shell always comes back, an allowlisted program is run
//! again, and anything else restores as a plain shell.

use std::path::{Path, PathBuf};

use crate::config::RestoreConfig;
use crate::model::Buffer;

/// Shells recognised by program name, so a pane that ran one restores as that
/// shell and can have its foreground program typed into it.
const KNOWN_SHELLS: &[&str] = &[
    "sh", "bash", "zsh", "fish", "dash", "ksh", "mksh", "oksh", "yash", "tcsh", "csh", "nu",
    "elvish", "xonsh", "pwsh",
];

/// How to bring back one pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RestorePlan {
    /// Command the pane's new keeper runs.
    pub command: Vec<String>,
    /// Working directory to start in: the last known one, or its nearest
    /// ancestor that still exists.
    pub cwd: Option<PathBuf>,
    /// Command line typed into the restored shell once it is up, to run the
    /// allowlisted program the pane had in the foreground.
    pub typed_command: Option<String>,
}

/// Decide how to restore `buffer`. `default_shell` is used when the pane ran a
/// program that is neither a shell nor allowlisted.
pub(crate) fn plan_restore(
    buffer: &Buffer,
    config: &RestoreConfig,
    default_shell: &[String],
) -> RestorePlan {
    let cwd = buffer.cwd.as_deref().and_then(nearest_existing_dir);
    let (command, typed_command) = if is_shell(&buffer.command) {
        let typed = buffer
            .foreground_command
            .as_deref()
            .filter(|argv| is_allowlisted(argv, config))
            .map(shell_command_line);
        (buffer.command.clone(), typed)
    } else if is_allowlisted(&buffer.command, config) {
        // The pane ran the program directly; run it directly again so it still
        // closes the pane when it exits.
        (buffer.command.clone(), None)
    } else {
        (default_shell.to_vec(), None)
    };
    RestorePlan {
        command,
        cwd,
        typed_command,
    }
}

/// The shell started for panes that ran something else: `$SHELL`, falling back
/// to `/bin/sh`, matching what the CLI starts new panes with.
pub(crate) fn default_shell() -> Vec<String> {
    vec![std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned())]
}

fn program_name(argv: &[String]) -> Option<&str> {
    let program = argv.first()?;
    let name = Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program);
    // Login shells are started with a leading dash, as in `-zsh`.
    Some(name.strip_prefix('-').unwrap_or(name))
}

fn is_shell(argv: &[String]) -> bool {
    program_name(argv).is_some_and(|name| KNOWN_SHELLS.contains(&name))
}

fn is_allowlisted(argv: &[String], config: &RestoreConfig) -> bool {
    program_name(argv).is_some_and(|name| config.commands.iter().any(|allowed| allowed == name))
}

fn nearest_existing_dir(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|candidate| candidate.is_dir())
        .map(Path::to_path_buf)
}

/// Join `argv` into a line a shell parses back into the same words. Arguments
/// with anything but plain characters are single-quoted, with quotes and
/// backslashes escaped outside the quotes so POSIX shells and fish agree.
fn shell_command_line(argv: &[String]) -> String {
    argv.iter()
        .map(|arg| shell_word(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_word(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-./:=@%+,".contains(c));
    if plain {
        return arg.to_owned();
    }
    let mut word = String::from("'");
    for c in arg.chars() {
        match c {
            '\'' => word.push_str("'\\''"),
            '\\' => word.push_str("'\\\\'"),
            c => word.push(c),
        }
    }
    word.push('\'');
    word
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use embers_core::BufferId;

    use super::*;

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    fn buffer(command: &[&str], foreground: Option<&[&str]>, cwd: Option<&Path>) -> Buffer {
        let mut buffer = Buffer::new(
            BufferId(1),
            "pane",
            argv(command),
            cwd.map(Path::to_path_buf),
            BTreeMap::new(),
        );
        buffer.foreground_command = foreground.map(argv);
        buffer
    }

    #[test]
    fn shell_pane_restores_the_shell_and_reruns_an_allowlisted_program() {
        let plan = plan_restore(
            &buffer(&["/bin/zsh"], Some(&["vim", "notes.txt"]), None),
            &RestoreConfig::default(),
            &argv(&["/bin/sh"]),
        );
        assert_eq!(plan.command, argv(&["/bin/zsh"]));
        assert_eq!(plan.typed_command.as_deref(), Some("vim notes.txt"));
    }

    #[test]
    fn shell_pane_does_not_rerun_a_program_off_the_allowlist() {
        let plan = plan_restore(
            &buffer(&["-bash"], Some(&["make", "deploy"]), None),
            &RestoreConfig::default(),
            &argv(&["/bin/sh"]),
        );
        assert_eq!(plan.command, argv(&["-bash"]));
        assert_eq!(plan.typed_command, None);
    }

    #[test]
    fn pane_running_a_program_directly_reruns_it_only_when_allowlisted() {
        let config = RestoreConfig::default();
        let shell = argv(&["/bin/sh"]);
        let plan = plan_restore(&buffer(&["/usr/bin/htop"], None, None), &config, &shell);
        assert_eq!(plan.command, argv(&["/usr/bin/htop"]));
        assert_eq!(plan.typed_command, None);

        let plan = plan_restore(&buffer(&["cargo", "watch"], None, None), &config, &shell);
        assert_eq!(plan.command, shell);
        assert_eq!(plan.typed_command, None);
    }

    #[test]
    fn allowlist_is_configurable() {
        let config = RestoreConfig {
            enabled: true,
            commands: argv(&["make"]),
        };
        let plan = plan_restore(
            &buffer(&["sh"], Some(&["make", "watch"]), None),
            &config,
            &argv(&["/bin/sh"]),
        );
        assert_eq!(plan.typed_command.as_deref(), Some("make watch"));
        let plan = plan_restore(
            &buffer(&["sh"], Some(&["vim"]), None),
            &config,
            &argv(&["/bin/sh"]),
        );
        assert_eq!(plan.typed_command, None);
    }

    #[test]
    fn cwd_falls_back_to_the_nearest_existing_ancestor() {
        let dir = tempfile::tempdir().expect("tempdir");
        let gone = dir.path().join("removed/deeper");
        let plan = plan_restore(
            &buffer(&["sh"], None, Some(&gone)),
            &RestoreConfig::default(),
            &argv(&["/bin/sh"]),
        );
        assert_eq!(plan.cwd.as_deref(), Some(dir.path()));
    }

    #[test]
    fn command_lines_quote_anything_a_shell_would_interpret() {
        assert_eq!(
            shell_command_line(&argv(&[
                "less", "-R", "a b.txt", "it's", "$(x)", "c\\d", ""
            ])),
            "less -R 'a b.txt' 'it'\\''s' '$(x)' 'c'\\\\'d' ''"
        );
    }
}
