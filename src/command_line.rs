use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::commands::{self, Command, Entry};

/// How many completions the prompt shows at once; the rest are scrolled to.
pub const VISIBLE_CANDIDATES: usize = 5;

pub enum Outcome {
    Continue,
    Cancel,
    Run(Command),
    Error(String),
}

/// The `:` prompt. Candidates are re-ranked on every edit and the highlighted one is what runs,
/// which is why an exact name or alias has to be pinned first by the search itself.
pub struct CommandLine {
    input: String,
    candidates: Vec<&'static Entry>,
    selected: usize,
    scroll: usize,
    history: Vec<String>,
    recalled: Option<usize>,
}

impl CommandLine {
    pub fn new() -> Self {
        Self {
            input: String::new(),
            candidates: commands::search(""),
            selected: 0,
            scroll: 0,
            history: Vec::new(),
            recalled: None,
        }
    }

    pub fn open(&mut self) {
        self.input.clear();
        self.recalled = None;
        self.refresh();
    }

    pub fn input(&self) -> &str {
        &self.input
    }

    pub fn candidates(&self) -> &[&'static Entry] {
        &self.candidates
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Index of the first completion on screen.
    pub fn scroll(&self) -> usize {
        self.scroll
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            KeyCode::Esc => Outcome::Cancel,
            KeyCode::Enter => self.run(),

            // The arrows scroll the completions, so history sits one modifier away.
            KeyCode::Up if ctrl => self.recall(true),
            KeyCode::Down if ctrl => self.recall(false),

            KeyCode::Down | KeyCode::Tab => self.highlight(1),
            KeyCode::Up | KeyCode::BackTab => self.highlight(-1),
            KeyCode::Char('n') if ctrl => self.highlight(1),
            KeyCode::Char('p') if ctrl => self.highlight(-1),

            KeyCode::Char('u') if ctrl => self.edit(|input| input.clear()),
            KeyCode::Char('w') if ctrl => self.edit(delete_word),

            // Nothing left but the `:` itself, so erasing it leaves the prompt.
            KeyCode::Backspace if self.input.is_empty() => Outcome::Cancel,
            KeyCode::Backspace => self.edit(|input| {
                input.pop();
            }),
            KeyCode::Char(char) => self.edit(|input| input.push(char)),

            _ => Outcome::Continue,
        }
    }

    fn edit(&mut self, change: impl FnOnce(&mut String)) -> Outcome {
        change(&mut self.input);
        self.recalled = None;
        self.refresh();

        Outcome::Continue
    }

    fn refresh(&mut self) {
        self.candidates = commands::search(&self.input);
        self.selected = 0;
        self.scroll = 0;
    }

    fn highlight(&mut self, delta: isize) -> Outcome {
        if self.candidates.is_empty() {
            return Outcome::Continue;
        }

        let count = self.candidates.len() as isize;
        let next = (self.selected as isize + delta).rem_euclid(count);
        self.selected = next as usize;
        self.follow_selection();

        Outcome::Continue
    }

    /// Moves the window the least amount needed to keep the highlighted entry on screen, so
    /// wrapping from the last candidate to the first also scrolls back to the top.
    fn follow_selection(&mut self) {
        if self.selected < self.scroll {
            self.scroll = self.selected;
        }
        if self.selected >= self.scroll + VISIBLE_CANDIDATES {
            self.scroll = self.selected + 1 - VISIBLE_CANDIDATES;
        }
    }

    /// History is oldest-first, so walking to an older entry counts down through the indices.
    fn recall(&mut self, older: bool) -> Outcome {
        if self.history.is_empty() {
            return Outcome::Continue;
        }

        let last = self.history.len() - 1;
        let index = match (self.recalled, older) {
            (None, true) => Some(last),
            (None, false) => None,
            (Some(current), true) => Some(current.saturating_sub(1)),
            (Some(current), false) if current == last => None,
            (Some(current), false) => Some(current + 1),
        };

        self.recalled = index;
        self.input = index.map_or_else(String::new, |index| self.history[index].clone());
        self.refresh();

        Outcome::Continue
    }

    fn run(&mut self) -> Outcome {
        if self.input.is_empty() {
            return Outcome::Cancel;
        }

        let Some(entry) = self.candidates.get(self.selected).copied() else {
            return Outcome::Error(format!("unknown command: {}", self.input));
        };

        self.remember(entry.name.to_owned());

        Outcome::Run(entry.command)
    }

    fn remember(&mut self, name: String) {
        self.history.retain(|previous| *previous != name);
        self.history.push(name);
    }
}

fn delete_word(input: &mut String) {
    let trimmed = input.trim_end_matches(|char: char| char.is_whitespace());
    let keep = trimmed
        .rfind(|char: char| char.is_whitespace() || char == '_')
        .map_or(0, |index| index + 1);

    input.truncate(keep);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(line: &mut CommandLine, key: KeyCode) -> Outcome {
        line.handle_key(KeyEvent::from(key))
    }

    fn press_ctrl(line: &mut CommandLine, key: KeyCode) -> Outcome {
        line.handle_key(KeyEvent::new(key, KeyModifiers::CONTROL))
    }

    fn type_text(line: &mut CommandLine, text: &str) {
        for char in text.chars() {
            press(line, KeyCode::Char(char));
        }
    }

    #[test]
    fn typing_narrows_the_candidates() {
        let mut line = CommandLine::new();
        type_text(&mut line, "cent");

        assert_eq!(line.candidates()[0].command, Command::CenterCursor);
    }

    #[test]
    fn enter_runs_the_highlighted_candidate() {
        let mut line = CommandLine::new();
        type_text(&mut line, "q");

        assert!(matches!(
            press(&mut line, KeyCode::Enter),
            Outcome::Run(Command::Quit)
        ));
    }

    #[test]
    fn tab_cycles_and_wraps() {
        let mut line = CommandLine::new();
        type_text(&mut line, "move");
        let count = line.candidates().len();

        for expected in 1..count {
            press(&mut line, KeyCode::Tab);
            assert_eq!(line.selected(), expected);
        }

        press(&mut line, KeyCode::Tab);
        assert_eq!(line.selected(), 0);

        press(&mut line, KeyCode::BackTab);
        assert_eq!(line.selected(), count - 1);
    }

    #[test]
    fn an_unknown_command_reports_an_error() {
        let mut line = CommandLine::new();
        type_text(&mut line, "zzzqqq");

        assert!(matches!(
            press(&mut line, KeyCode::Enter),
            Outcome::Error(_)
        ));
    }

    #[test]
    fn escape_and_an_empty_prompt_both_cancel() {
        let mut line = CommandLine::new();

        assert!(matches!(press(&mut line, KeyCode::Enter), Outcome::Cancel));
        assert!(matches!(press(&mut line, KeyCode::Esc), Outcome::Cancel));
    }

    #[test]
    fn backspacing_the_prompt_away_cancels() {
        let mut line = CommandLine::new();
        type_text(&mut line, "qu");

        assert!(matches!(
            press(&mut line, KeyCode::Backspace),
            Outcome::Continue
        ));
        assert_eq!(line.input(), "q");

        assert!(matches!(
            press(&mut line, KeyCode::Backspace),
            Outcome::Continue
        ));
        assert_eq!(line.input(), "");

        assert!(matches!(
            press(&mut line, KeyCode::Backspace),
            Outcome::Cancel
        ));
    }

    #[test]
    fn history_recalls_the_last_command_first() {
        let mut line = CommandLine::new();

        type_text(&mut line, "help");
        press(&mut line, KeyCode::Enter);
        line.open();
        type_text(&mut line, "quit");
        press(&mut line, KeyCode::Enter);
        line.open();

        press_ctrl(&mut line, KeyCode::Up);
        assert_eq!(line.input(), "quit");

        press_ctrl(&mut line, KeyCode::Up);
        assert_eq!(line.input(), "help");

        press_ctrl(&mut line, KeyCode::Down);
        assert_eq!(line.input(), "quit");

        press_ctrl(&mut line, KeyCode::Down);
        assert_eq!(line.input(), "");
    }

    #[test]
    fn the_arrows_move_the_highlight() {
        let mut line = CommandLine::new();

        press(&mut line, KeyCode::Down);
        assert_eq!(line.selected(), 1);

        press(&mut line, KeyCode::Up);
        assert_eq!(line.selected(), 0);
    }

    #[test]
    fn the_window_follows_the_highlight() {
        let mut line = CommandLine::new();
        assert!(
            line.candidates().len() > VISIBLE_CANDIDATES,
            "the list has to be longer than the window for this to mean anything"
        );

        for _ in 0..VISIBLE_CANDIDATES - 1 {
            press(&mut line, KeyCode::Down);
        }
        assert_eq!(line.scroll(), 0, "no scrolling until the window is full");

        press(&mut line, KeyCode::Down);
        assert_eq!(line.selected(), VISIBLE_CANDIDATES);
        assert_eq!(line.scroll(), 1);

        press(&mut line, KeyCode::Up);
        assert_eq!(
            line.scroll(),
            1,
            "moving back inside the window must not scroll"
        );

        for _ in 0..VISIBLE_CANDIDATES - 1 {
            press(&mut line, KeyCode::Up);
        }
        assert_eq!(line.selected(), 0);
        assert_eq!(line.scroll(), 0);
    }

    #[test]
    fn wrapping_to_the_last_candidate_scrolls_to_the_end() {
        let mut line = CommandLine::new();
        let last = line.candidates().len() - 1;

        press(&mut line, KeyCode::Up);

        assert_eq!(line.selected(), last);
        assert_eq!(line.scroll(), last + 1 - VISIBLE_CANDIDATES);
    }

    #[test]
    fn editing_returns_to_the_top_of_the_list() {
        let mut line = CommandLine::new();
        for _ in 0..VISIBLE_CANDIDATES + 2 {
            press(&mut line, KeyCode::Down);
        }
        assert!(line.scroll() > 0);

        type_text(&mut line, "m");

        assert_eq!(line.selected(), 0);
        assert_eq!(line.scroll(), 0);
    }

    #[test]
    fn control_w_deletes_a_word_and_control_u_clears() {
        let mut line = CommandLine::new();
        type_text(&mut line, "move_next_sibling");

        line.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(line.input(), "move_next_");

        line.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(line.input(), "");
    }
}
