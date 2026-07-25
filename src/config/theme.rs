use ratatui::style::Color;
use serde::de::{Deserialize, Deserializer, Error};

#[derive(Debug, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Theme {
    pub file: Paint,
    pub file_modified: Paint,
    pub file_staged: Paint,
    pub file_untracked: Paint,
    pub file_ignored: Paint,
    pub file_conflicted: Paint,
    pub dir: Paint,
    pub dir_changed: Paint,
    pub diff_added: Paint,
    pub diff_removed: Paint,
    pub ui_selection_bg: Paint,
    pub ui_status_bar_bg: Paint,
    pub ui_status_bar_fg: Paint,
    pub ui_hint: Paint,
}

/// ANSI names rather than hex, so the defaults follow whatever palette the terminal is themed with.
impl Default for Theme {
    fn default() -> Self {
        Self {
            file: Paint(Color::Reset),
            file_modified: Paint(Color::Yellow),
            file_staged: Paint(Color::Green),
            file_untracked: Paint(Color::Green),
            file_ignored: Paint(Color::Gray),
            file_conflicted: Paint(Color::LightRed),
            dir: Paint(Color::Blue),
            dir_changed: Paint(Color::Yellow),
            diff_added: Paint(Color::Green),
            diff_removed: Paint(Color::Red),
            ui_selection_bg: Paint(Color::DarkGray),
            ui_status_bar_bg: Paint(Color::Black),
            ui_status_bar_fg: Paint(Color::White),
            ui_hint: Paint(Color::Gray),
        }
    }
}

/// A colour written as `"#rrggbb"`, an ANSI name like `"blue"`, or `"reset"`.
#[derive(Clone, Copy, Debug)]
pub struct Paint(pub Color);

impl<'de> Deserialize<'de> for Paint {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;

        parse(&raw)
            .map(Paint)
            .ok_or_else(|| D::Error::custom(format!("unknown colour: {raw}")))
    }
}

fn parse(raw: &str) -> Option<Color> {
    if let Some(hex) = raw.strip_prefix('#') {
        if hex.len() != 6 {
            return None;
        }

        let channel = |range: std::ops::Range<usize>| u8::from_str_radix(&hex[range], 16).ok();

        return Some(Color::Rgb(channel(0..2)?, channel(2..4)?, channel(4..6)?));
    }

    let color = match raw {
        "reset" => Color::Reset,
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "gray" | "grey" => Color::Gray,
        "dark_gray" | "dark_grey" => Color::DarkGray,
        "light_red" => Color::LightRed,
        "light_green" => Color::LightGreen,
        "light_yellow" => Color::LightYellow,
        "light_blue" => Color::LightBlue,
        "light_magenta" => Color::LightMagenta,
        "light_cyan" => Color::LightCyan,
        "white" => Color::White,
        _ => return None,
    };

    Some(color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_and_names() {
        assert_eq!(parse("#73c991"), Some(Color::Rgb(0x73, 0xc9, 0x91)));
        assert_eq!(parse("reset"), Some(Color::Reset));
        assert_eq!(parse("light_blue"), Some(Color::LightBlue));
    }

    #[test]
    fn rejects_malformed_colours() {
        assert_eq!(parse("#abc"), None);
        assert_eq!(parse("#gggggg"), None);
        assert_eq!(parse("chartreuse"), None);
    }
}
