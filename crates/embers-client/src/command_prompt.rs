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

use crate::controller::KeyEvent;

/// Most command lines the prompt remembers.
pub const PROMPT_HISTORY_LIMIT: usize = 100;

/// The keys prompts are edited with, like tmux's `status-keys`
/// (`set_status_keys`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StatusKeys {
    /// Emacs-style keys only (C-a, C-e, C-w, ...).
    #[default]
    Emacs,
    /// Emacs-style keys while inserting, and a vi normal mode after Esc.
    Vi,
}

impl StatusKeys {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "emacs" => Some(Self::Emacs),
            "vi" => Some(Self::Vi),
            _ => None,
        }
    }
}

/// The mode of a prompt edited with vi keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViMode {
    /// Typed text goes into the line.
    Insert,
    /// Keys are commands (`h`, `w`, `x`, `dw`, `i`, ...).
    Normal,
}

/// Vi editing state of a prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ViState {
    mode: ViMode,
    /// An operator (`d` or `c`) waiting for its motion.
    pending: Option<char>,
}

/// What a character is, for vi word motions: runs of word characters and runs
/// of other non-blank characters are words, as in vi.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Blank,
    Word,
    Punctuation,
}

fn char_class(ch: char) -> CharClass {
    if ch.is_whitespace() {
        CharClass::Blank
    } else if ch.is_alphanumeric() || ch == '_' {
        CharClass::Word
    } else {
        CharClass::Punctuation
    }
}

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
    /// Vi editing state, when the prompt uses vi keys.
    vi: Option<ViState>,
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
            vi: None,
        }
    }

    /// Edit with vi keys (`status-keys vi`), starting in insert mode.
    pub fn with_vi_keys(mut self) -> Self {
        self.vi = Some(ViState {
            mode: ViMode::Insert,
            pending: None,
        });
        self
    }

    /// The vi mode, or `None` for a prompt edited with emacs keys only.
    pub fn vi_mode(&self) -> Option<ViMode> {
        self.vi.map(|vi| vi.mode)
    }

    /// Handle `key` with the vi key table. Returns `false` for keys the
    /// table leaves to the default prompt keys: every key in insert mode
    /// except Esc, which switches to normal mode, and in normal mode Enter,
    /// Esc (which cancels), C-c, C-g and the arrow, Home, End, Backspace and
    /// Delete keys. Other keys in normal mode are commands, or ignored.
    pub fn vi_key(&mut self, key: &KeyEvent, history: &VecDeque<String>) -> bool {
        let Some(vi) = self.vi else {
            return false;
        };
        match (vi.mode, key) {
            (ViMode::Insert, KeyEvent::Escape) => {
                self.set_vi(ViMode::Normal);
                self.left();
                true
            }
            (ViMode::Insert, _) => false,
            (ViMode::Normal, KeyEvent::Char(ch)) => {
                match vi.pending {
                    Some(operator) => self.vi_operator(operator, *ch),
                    None => self.vi_command(*ch, history),
                }
                self.clamp_normal_cursor();
                true
            }
            (
                ViMode::Normal,
                KeyEvent::Enter
                | KeyEvent::Escape
                | KeyEvent::Ctrl('c' | 'g')
                | KeyEvent::Left
                | KeyEvent::Right
                | KeyEvent::Up
                | KeyEvent::Down
                | KeyEvent::Home
                | KeyEvent::End
                | KeyEvent::Backspace
                | KeyEvent::Delete,
            ) => {
                self.set_vi(ViMode::Normal);
                false
            }
            (ViMode::Normal, _) => {
                self.set_vi(ViMode::Normal);
                true
            }
        }
    }

    fn set_vi(&mut self, mode: ViMode) {
        self.vi = Some(ViState {
            mode,
            pending: None,
        });
    }

    fn vi_command(&mut self, ch: char, history: &VecDeque<String>) {
        match ch {
            'h' => self.left(),
            'l' => self.right(),
            '0' => self.home(),
            '$' => self.end(),
            'w' => self.cursor = self.next_word_start(),
            'b' => self.cursor = self.previous_word_start(),
            'e' => self.cursor = self.next_word_end(),
            'x' => self.delete(),
            'D' => self.kill_to_end(),
            'C' => {
                self.kill_to_end();
                self.set_vi(ViMode::Insert);
            }
            'i' => self.set_vi(ViMode::Insert),
            'a' => {
                self.right();
                self.set_vi(ViMode::Insert);
            }
            'A' => {
                self.end();
                self.set_vi(ViMode::Insert);
            }
            'I' => {
                self.home();
                self.set_vi(ViMode::Insert);
            }
            'k' => self.history_up(history),
            'j' => self.history_down(history),
            'd' | 'c' => {
                self.vi = Some(ViState {
                    mode: ViMode::Normal,
                    pending: Some(ch),
                });
            }
            _ => {}
        }
    }

    /// Apply `operator` (`d` or `c`) with the motion `motion`. Only `w` is a
    /// motion: `dw` deletes to the next word, `cw`, as in vi, changes to the
    /// end of the word.
    fn vi_operator(&mut self, operator: char, motion: char) {
        self.set_vi(ViMode::Normal);
        if motion != 'w' {
            return;
        }
        let end = if operator == 'c' && !self.char_at(self.cursor).is_none_or(char::is_whitespace) {
            self.word_end_from(self.cursor) + 1
        } else {
            self.next_word_start()
        };
        let from = self.byte_index(self.cursor);
        let to = self.byte_index(end);
        self.input.replace_range(from..to, "");
        if operator == 'c' {
            self.set_vi(ViMode::Insert);
        }
    }

    /// Keep the cursor on a character in normal mode, as vi does.
    fn clamp_normal_cursor(&mut self) {
        if self.vi_mode() == Some(ViMode::Normal) {
            self.cursor = self
                .cursor
                .min(self.input.chars().count().saturating_sub(1));
        }
    }

    fn char_at(&self, index: usize) -> Option<char> {
        self.input.chars().nth(index)
    }

    /// `w`: the start of the next word, or the end of the line.
    fn next_word_start(&self) -> usize {
        let chars: Vec<char> = self.input.chars().collect();
        let mut index = self.cursor;
        if let Some(&ch) = chars.get(index) {
            let class = char_class(ch);
            if class != CharClass::Blank {
                while index < chars.len() && char_class(chars[index]) == class {
                    index += 1;
                }
            }
        }
        while index < chars.len() && char_class(chars[index]) == CharClass::Blank {
            index += 1;
        }
        index
    }

    /// `b`: the start of this word, or of the previous one when already there.
    fn previous_word_start(&self) -> usize {
        let chars: Vec<char> = self.input.chars().collect();
        let mut index = self.cursor.min(chars.len());
        while index > 0 && char_class(chars[index - 1]) == CharClass::Blank {
            index -= 1;
        }
        if index == 0 {
            return 0;
        }
        let class = char_class(chars[index - 1]);
        while index > 0 && char_class(chars[index - 1]) == class {
            index -= 1;
        }
        index
    }

    /// `e`: the end of this word, or of the next one when already there.
    fn next_word_end(&self) -> usize {
        let chars: Vec<char> = self.input.chars().collect();
        let mut index = self.cursor + 1;
        while index < chars.len() && char_class(chars[index]) == CharClass::Blank {
            index += 1;
        }
        if index >= chars.len() {
            return chars.len().saturating_sub(1);
        }
        self.word_end_from(index)
    }

    /// The last character of the word that `index` is in.
    fn word_end_from(&self, index: usize) -> usize {
        let chars: Vec<char> = self.input.chars().collect();
        let Some(&ch) = chars.get(index) else {
            return index;
        };
        let class = char_class(ch);
        let mut end = index;
        while end + 1 < chars.len() && char_class(chars[end + 1]) == class {
            end += 1;
        }
        end
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

    fn vi_prompt(initial: &str) -> CommandPrompt {
        CommandPrompt::new(":", initial, None, "normal").with_vi_keys()
    }

    /// Feed `keys` to the prompt as vi keys, the way the client does: keys
    /// the vi table does not take get the default editing.
    fn type_vi(prompt: &mut CommandPrompt, keys: &str, history: &VecDeque<String>) {
        for ch in keys.chars() {
            let key = if ch == '\u{1b}' {
                KeyEvent::Escape
            } else {
                KeyEvent::Char(ch)
            };
            if !prompt.vi_key(&key, history) {
                match key {
                    KeyEvent::Char(ch) => prompt.insert(ch.encode_utf8(&mut [0; 4])),
                    other => panic!("vi table passed on {other:?}"),
                }
            }
        }
    }

    #[test]
    fn vi_keys_start_in_insert_mode_and_escape_to_normal_mode() {
        let history = VecDeque::new();
        let mut prompt = vi_prompt("");
        assert_eq!(prompt.vi_mode(), Some(ViMode::Insert));
        type_vi(&mut prompt, "kill-pane", &history);
        assert_eq!(prompt.input, "kill-pane");

        // Esc goes to normal mode and steps back onto the last character.
        type_vi(&mut prompt, "\u{1b}", &history);
        assert_eq!(prompt.vi_mode(), Some(ViMode::Normal));
        assert_eq!(prompt.cursor, 8);
        // Normal-mode keys edit rather than insert.
        type_vi(&mut prompt, "0x", &history);
        assert_eq!(prompt.input, "ill-pane");
        type_vi(&mut prompt, "$x", &history);
        assert_eq!(prompt.input, "ill-pan");
        assert_eq!(prompt.cursor, 6);
        // Unbound normal-mode keys do nothing.
        type_vi(&mut prompt, "z", &history);
        assert_eq!(prompt.input, "ill-pan");

        // Esc in normal mode is left to the client, which cancels.
        assert!(!prompt.vi_key(&KeyEvent::Escape, &history));
        // So are Enter and C-c.
        assert!(!prompt.vi_key(&KeyEvent::Enter, &history));
        assert!(!prompt.vi_key(&KeyEvent::Ctrl('c'), &history));
    }

    #[test]
    fn vi_insert_commands_place_the_cursor() {
        let history = VecDeque::new();
        let mut prompt = vi_prompt("split-window");
        type_vi(&mut prompt, "\u{1b}0iX\u{1b}", &history);
        assert_eq!(prompt.input, "Xsplit-window");
        type_vi(&mut prompt, "aY\u{1b}", &history);
        assert_eq!(prompt.input, "XYsplit-window");
        type_vi(&mut prompt, "A -h\u{1b}", &history);
        assert_eq!(prompt.input, "XYsplit-window -h");
        type_vi(&mut prompt, "I: \u{1b}", &history);
        assert_eq!(prompt.input, ": XYsplit-window -h");
        // h and l move within the line.
        type_vi(&mut prompt, "lllhx", &history);
        assert_eq!(prompt.input, ": Xsplit-window -h");
    }

    #[test]
    fn vi_word_motions_and_operators() {
        let history = VecDeque::new();
        let mut prompt = vi_prompt("new-window --title logs");
        type_vi(&mut prompt, "\u{1b}0", &history);
        // `-` is punctuation, so words are "new", "-", "window", "--", ...
        type_vi(&mut prompt, "w", &history);
        assert_eq!(prompt.cursor, 3);
        type_vi(&mut prompt, "ww", &history);
        assert_eq!(prompt.cursor, 11);
        type_vi(&mut prompt, "e", &history);
        assert_eq!(prompt.cursor, 12);
        type_vi(&mut prompt, "b", &history);
        assert_eq!(prompt.cursor, 11);
        type_vi(&mut prompt, "b", &history);
        assert_eq!(prompt.cursor, 4);

        // dw deletes to the next word; cw changes to the end of the word.
        type_vi(&mut prompt, "dw", &history);
        assert_eq!(prompt.input, "new---title logs");
        type_vi(&mut prompt, "$bcwtail\u{1b}", &history);
        assert_eq!(prompt.input, "new---title tail");
        assert_eq!(prompt.vi_mode(), Some(ViMode::Normal));

        // D deletes to the end; C changes to the end.
        type_vi(&mut prompt, "0wD", &history);
        assert_eq!(prompt.input, "new");
        type_vi(&mut prompt, "0Cfoo\u{1b}", &history);
        assert_eq!(prompt.input, "foo");
        // An operator followed by an unknown motion is dropped.
        type_vi(&mut prompt, "dzx", &history);
        assert_eq!(prompt.input, "fo");
    }

    #[test]
    fn vi_j_and_k_browse_history() {
        let mut history = VecDeque::new();
        record_history(&mut history, "new-window");
        record_history(&mut history, "kill-pane");
        let mut prompt = vi_prompt("sel");
        type_vi(&mut prompt, "\u{1b}k", &history);
        assert_eq!(prompt.input, "kill-pane");
        // The cursor stays on a character in normal mode.
        assert_eq!(prompt.cursor, 8);
        type_vi(&mut prompt, "k", &history);
        assert_eq!(prompt.input, "new-window");
        type_vi(&mut prompt, "jj", &history);
        assert_eq!(prompt.input, "sel");
    }

    #[test]
    fn emacs_prompts_leave_every_key_to_the_client() {
        let history = VecDeque::new();
        let mut prompt = prompt("x");
        assert_eq!(prompt.vi_mode(), None);
        assert!(!prompt.vi_key(&KeyEvent::Escape, &history));
        assert!(!prompt.vi_key(&KeyEvent::Char('h'), &history));
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
