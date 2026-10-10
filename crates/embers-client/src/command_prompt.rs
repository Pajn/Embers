//! The command prompt (tmux `:`): line editing, history and the hook that
//! runs a command line.
//!
//! The client only collects the line. Running it is up to a [`CommandRunner`],
//! which the `embers` binary provides by parsing the words with the same
//! definitions as its CLI, so the prompt and the CLI accept the same commands.

use std::collections::VecDeque;

use async_trait::async_trait;
use embers_core::{Result, SessionId};
use unicode_width::UnicodeWidthStr;

/// Most command lines the prompt remembers.
pub const PROMPT_HISTORY_LIMIT: usize = 100;

/// What a command run from the prompt applies to when it names no target.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandContext {
    /// The client's current session: the default session, and through it the
    /// default window and pane.
    pub session_id: Option<SessionId>,
    /// This client, the default for client commands such as `display-message`.
    pub client_id: Option<u64>,
    /// The client's window base index.
    pub base_index: Option<u32>,
}

/// Runs a command line typed at the prompt, already split into words, and
/// returns its output.
#[async_trait]
pub trait CommandRunner: Send + Sync {
    async fn run(&self, argv: Vec<String>, context: CommandContext) -> Result<String>;
}

/// An open command prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandPrompt {
    /// Text shown before the input, such as `:` or `(rename-window) `.
    pub label: String,
    pub input: String,
    /// Cursor position in `input`, in characters.
    pub cursor: usize,
    /// A command line in which `%%` stands for the input, quoted as one word
    /// (tmux `command-prompt "rename-window '%%'"`). Without one the input is
    /// the command line.
    pub template: Option<String>,
    /// Mode to return to when the prompt closes.
    pub return_mode: String,
    /// The history entry shown, while browsing with Up/Down.
    history_index: Option<usize>,
    /// The input typed before browsing history, restored past the newest entry.
    draft: String,
}

impl CommandPrompt {
    pub fn new(
        label: impl Into<String>,
        initial: impl Into<String>,
        template: Option<String>,
        return_mode: impl Into<String>,
    ) -> Self {
        let input = initial.into();
        Self {
            label: label.into(),
            cursor: input.chars().count(),
            input,
            template,
            return_mode: return_mode.into(),
            history_index: None,
            draft: String::new(),
        }
    }

    /// The prompt as shown in the status line.
    pub fn display(&self) -> String {
        format!("{}{}", self.label, self.input)
    }

    /// Display column of the cursor within [`Self::display`].
    pub fn cursor_column(&self) -> usize {
        let before: String = self.input.chars().take(self.cursor).collect();
        UnicodeWidthStr::width(self.label.as_str()) + UnicodeWidthStr::width(before.as_str())
    }

    /// The command line to run: the input, or the template with `%%`
    /// replaced by the quoted input.
    pub fn command_line(&self) -> String {
        match &self.template {
            Some(template) => template.replace("%%", &shell_words::quote(&self.input)),
            None => self.input.clone(),
        }
    }

    pub fn insert(&mut self, text: &str) {
        let at = self.byte_index(self.cursor);
        // Keep the line on one line: newlines and other controls become spaces.
        let text: String = text
            .chars()
            .map(|ch| if ch.is_control() { ' ' } else { ch })
            .collect();
        self.input.insert_str(at, &text);
        self.cursor += text.chars().count();
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = self.byte_index(self.cursor - 1);
        let end = self.byte_index(self.cursor);
        self.input.replace_range(start..end, "");
        self.cursor -= 1;
    }

    pub fn delete(&mut self) {
        if self.cursor >= self.input.chars().count() {
            return;
        }
        let start = self.byte_index(self.cursor);
        let end = self.byte_index(self.cursor + 1);
        self.input.replace_range(start..end, "");
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.input.chars().count());
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.input.chars().count();
    }

    /// Delete from the start of the line to the cursor (C-u).
    pub fn kill_to_start(&mut self) {
        let end = self.byte_index(self.cursor);
        self.input.replace_range(..end, "");
        self.cursor = 0;
    }

    /// Delete from the cursor to the end of the line (C-k).
    pub fn kill_to_end(&mut self) {
        let start = self.byte_index(self.cursor);
        self.input.truncate(start);
    }

    /// Delete the word before the cursor (C-w).
    pub fn delete_word(&mut self) {
        let chars: Vec<char> = self.input.chars().collect();
        let mut start = self.cursor;
        while start > 0 && chars[start - 1].is_whitespace() {
            start -= 1;
        }
        while start > 0 && !chars[start - 1].is_whitespace() {
            start -= 1;
        }
        let from = self.byte_index(start);
        let to = self.byte_index(self.cursor);
        self.input.replace_range(from..to, "");
        self.cursor = start;
    }

    /// Show the previous (older) history entry.
    pub fn history_up(&mut self, history: &VecDeque<String>) {
        let next = match self.history_index {
            None if history.is_empty() => return,
            None => {
                self.draft = self.input.clone();
                history.len() - 1
            }
            Some(0) => return,
            Some(index) => index - 1,
        };
        self.history_index = Some(next);
        self.set_input(history[next].clone());
    }

    /// Show the next (newer) history entry, or the draft past the newest.
    pub fn history_down(&mut self, history: &VecDeque<String>) {
        let Some(index) = self.history_index else {
            return;
        };
        if index + 1 < history.len() {
            self.history_index = Some(index + 1);
            self.set_input(history[index + 1].clone());
        } else {
            self.history_index = None;
            let draft = std::mem::take(&mut self.draft);
            self.set_input(draft);
        }
    }

    fn set_input(&mut self, input: String) {
        self.cursor = input.chars().count();
        self.input = input;
    }

    fn byte_index(&self, chars: usize) -> usize {
        self.input
            .char_indices()
            .nth(chars)
            .map_or(self.input.len(), |(index, _)| index)
    }
}

/// Remember `line` in `history`, skipping blanks and repeats of the newest.
pub fn record_history(history: &mut VecDeque<String>, line: &str) {
    if line.trim().is_empty() || history.back().is_some_and(|last| last == line) {
        return;
    }
    history.push_back(line.to_owned());
    while history.len() > PROMPT_HISTORY_LIMIT {
        history.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompt(initial: &str) -> CommandPrompt {
        CommandPrompt::new(":", initial, None, "normal")
    }

    #[test]
    fn edits_at_the_cursor() {
        let mut prompt = prompt("slit-window");
        prompt.home();
        prompt.right();
        prompt.insert("p");
        assert_eq!(prompt.input, "split-window");
        prompt.end();
        prompt.insert(" -h");
        prompt.backspace();
        prompt.insert("v");
        assert_eq!(prompt.input, "split-window -v");
        prompt.delete_word();
        assert_eq!(prompt.input, "split-window ");
        prompt.home();
        prompt.delete();
        assert_eq!(prompt.input, "plit-window ");
        prompt.right();
        prompt.kill_to_end();
        assert_eq!(prompt.input, "p");
        prompt.kill_to_start();
        assert_eq!(prompt.input, "");
        assert_eq!(prompt.cursor, 0);
    }

    #[test]
    fn handles_multibyte_text_and_controls() {
        let mut prompt = prompt("åä");
        prompt.left();
        prompt.insert("x\ny");
        assert_eq!(prompt.input, "åx yä");
        assert_eq!(prompt.cursor_column(), 1 + 4);
        prompt.backspace();
        assert_eq!(prompt.input, "åx ä");
    }

    #[test]
    fn templates_quote_the_input_as_one_word() {
        let prompt = CommandPrompt::new(
            "(rename-window) ",
            "it's mine",
            Some("rename-window -- %%".to_owned()),
            "normal",
        );
        let line = prompt.command_line();
        assert_eq!(
            shell_words::split(&line).unwrap(),
            vec!["rename-window", "--", "it's mine"]
        );
        assert_eq!(prompt.display(), "(rename-window) it's mine");
    }

    #[test]
    fn history_browses_older_and_back_to_the_draft() {
        let mut history = VecDeque::new();
        record_history(&mut history, "new-window");
        record_history(&mut history, "new-window");
        record_history(&mut history, "  ");
        record_history(&mut history, "kill-pane");
        assert_eq!(history, ["new-window", "kill-pane"]);

        let mut prompt = prompt("sel");
        prompt.history_up(&history);
        assert_eq!(prompt.input, "kill-pane");
        prompt.history_up(&history);
        prompt.history_up(&history);
        assert_eq!(prompt.input, "new-window");
        prompt.history_down(&history);
        assert_eq!(prompt.input, "kill-pane");
        prompt.history_down(&history);
        assert_eq!(prompt.input, "sel");
        assert_eq!(prompt.cursor, 3);
    }

    #[test]
    fn history_is_capped() {
        let mut history = VecDeque::new();
        for index in 0..PROMPT_HISTORY_LIMIT + 5 {
            record_history(&mut history, &format!("cmd {index}"));
        }
        assert_eq!(history.len(), PROMPT_HISTORY_LIMIT);
        assert_eq!(history.front().map(String::as_str), Some("cmd 5"));
    }
}
