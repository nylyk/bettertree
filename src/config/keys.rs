use std::collections::HashMap;
use std::fmt;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::de::{Deserialize, Deserializer, Error};

use crate::commands::Command;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct KeyBinding {
    code: KeyCode,
    modifiers: KeyModifiers,
}

impl KeyBinding {
    /// Shift is dropped for character keys because the character itself already carries it.
    pub fn from_event(event: KeyEvent) -> Self {
        let modifiers = match event.code {
            KeyCode::Char(_) => event.modifiers - KeyModifiers::SHIFT,
            _ => event.modifiers,
        };

        Self {
            code: event.code,
            modifiers,
        }
    }

    pub fn parse(spec: &str) -> Result<Self, String> {
        let mut rest = spec;
        let mut modifiers = KeyModifiers::NONE;

        while rest.len() > 2 {
            let modifier = match &rest[..2] {
                "C-" => KeyModifiers::CONTROL,
                "S-" => KeyModifiers::SHIFT,
                "A-" => KeyModifiers::ALT,
                _ => break,
            };

            modifiers |= modifier;
            rest = &rest[2..];
        }

        let code = parse_code(rest).ok_or_else(|| format!("unknown key: {spec}"))?;

        // `S-tab` is what terminals report as back-tab, and shift on a character is the character.
        let binding = match (code, modifiers.contains(KeyModifiers::SHIFT)) {
            (KeyCode::Tab, true) => Self {
                code: KeyCode::BackTab,
                modifiers: modifiers - KeyModifiers::SHIFT,
            },
            (KeyCode::Char(_), true) => {
                return Err(format!("write shifted characters directly, not as {spec}"));
            }
            _ => Self { code, modifiers },
        };

        Ok(binding)
    }
}

fn parse_code(name: &str) -> Option<KeyCode> {
    let code = match name {
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pgup" => KeyCode::PageUp,
        "pgdn" => KeyCode::PageDown,
        "enter" => KeyCode::Enter,
        "tab" => KeyCode::Tab,
        "space" => KeyCode::Char(' '),
        "backspace" => KeyCode::Backspace,
        "esc" => KeyCode::Esc,
        "del" => KeyCode::Delete,
        "insert" => KeyCode::Insert,
        _ => {
            let mut chars = name.chars();
            let first = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            KeyCode::Char(first)
        }
    };

    Some(code)
}

impl fmt::Display for KeyBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            f.write_str("C-")?;
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            f.write_str("A-")?;
        }

        let name = match self.code {
            KeyCode::Up => "up",
            KeyCode::Down => "down",
            KeyCode::Left => "left",
            KeyCode::Right => "right",
            KeyCode::Home => "home",
            KeyCode::End => "end",
            KeyCode::PageUp => "pgup",
            KeyCode::PageDown => "pgdn",
            KeyCode::Enter => "enter",
            KeyCode::Tab => "tab",
            KeyCode::BackTab => "S-tab",
            KeyCode::Backspace => "backspace",
            KeyCode::Esc => "esc",
            KeyCode::Delete => "del",
            KeyCode::Insert => "insert",
            KeyCode::Char(' ') => "space",
            KeyCode::Char(char) => return write!(f, "{char}"),
            other => return write!(f, "{other:?}"),
        };

        f.write_str(name)
    }
}

#[derive(Default)]
pub struct Keymap(HashMap<KeyBinding, Command>);

impl Keymap {
    pub fn get(&self, event: KeyEvent) -> Option<Command> {
        self.0.get(&KeyBinding::from_event(event)).copied()
    }

    /// The key specs bound to a command, sorted so the help overlay is stable between runs.
    pub fn keys_for(&self, command: Command) -> Vec<String> {
        let mut specs: Vec<String> = self
            .0
            .iter()
            .filter(|(_, bound)| **bound == command)
            .map(|(binding, _)| binding.to_string())
            .collect();

        specs.sort_unstable();

        specs
    }
}

impl fmt::Debug for Keymap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.0.iter()).finish()
    }
}

impl<'de> Deserialize<'de> for Keymap {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = HashMap::<String, String>::deserialize(deserializer)?;
        let mut keymap = HashMap::with_capacity(raw.len());

        for (spec, name) in raw {
            let binding = KeyBinding::parse(&spec).map_err(D::Error::custom)?;
            let command = Command::from_name(&name)
                .ok_or_else(|| D::Error::custom(format!("unknown command: {name}")))?;

            keymap.insert(binding, command);
        }

        Ok(Keymap(keymap))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(code: KeyCode, modifiers: KeyModifiers) -> KeyBinding {
        KeyBinding { code, modifiers }
    }

    #[test]
    fn parses_plain_keys() {
        assert_eq!(
            KeyBinding::parse("j"),
            Ok(binding(KeyCode::Char('j'), KeyModifiers::NONE))
        );
        assert_eq!(
            KeyBinding::parse("pgdn"),
            Ok(binding(KeyCode::PageDown, KeyModifiers::NONE))
        );
        assert_eq!(
            KeyBinding::parse("space"),
            Ok(binding(KeyCode::Char(' '), KeyModifiers::NONE))
        );
    }

    #[test]
    fn parses_modifiers() {
        assert_eq!(
            KeyBinding::parse("C-d"),
            Ok(binding(KeyCode::Char('d'), KeyModifiers::CONTROL))
        );
        assert_eq!(
            KeyBinding::parse("A-enter"),
            Ok(binding(KeyCode::Enter, KeyModifiers::ALT))
        );
        assert_eq!(
            KeyBinding::parse("C-A-up"),
            Ok(binding(
                KeyCode::Up,
                KeyModifiers::CONTROL | KeyModifiers::ALT
            ))
        );
    }

    #[test]
    fn parses_a_literal_dash() {
        assert_eq!(
            KeyBinding::parse("-"),
            Ok(binding(KeyCode::Char('-'), KeyModifiers::NONE))
        );
        assert_eq!(
            KeyBinding::parse("C--"),
            Ok(binding(KeyCode::Char('-'), KeyModifiers::CONTROL))
        );
    }

    #[test]
    fn shift_tab_becomes_back_tab() {
        assert_eq!(
            KeyBinding::parse("S-tab"),
            Ok(binding(KeyCode::BackTab, KeyModifiers::NONE))
        );
    }

    #[test]
    fn rejects_unknown_and_shifted_characters() {
        assert!(KeyBinding::parse("pgdown").is_err());
        assert!(KeyBinding::parse("C-nope").is_err());
        assert!(KeyBinding::parse("S-a").is_err());
    }

    #[test]
    fn uppercase_events_match_uppercase_bindings() {
        let event = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT);

        assert_eq!(
            KeyBinding::from_event(event),
            KeyBinding::parse("G").expect("parses")
        );
    }
}
