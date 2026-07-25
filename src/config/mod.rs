pub mod keys;
pub mod theme;

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use etcetera::{BaseStrategy, choose_base_strategy};
use serde::{Deserialize, Serialize};

use crate::tree::scan::SortOrder;
use keys::Keymap;
use theme::Theme;

const DEFAULT_CONFIG: &str = include_str!("default_config.toml");

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub editor: String,
    pub scroll_lines: usize,
    pub scrolloff: usize,
    pub icons: Icons,
    pub sort_order: SortOrder,
    pub show_diffstat: bool,
    pub indent: usize,
    pub watch: bool,
    pub toggles: Toggles,
    pub colors: Theme,
    pub keys: Keymap,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            editor: String::new(),
            scroll_lines: 10,
            scrolloff: 3,
            icons: Icons::default(),
            sort_order: SortOrder::default(),
            show_diffstat: true,
            indent: 2,
            watch: true,
            toggles: Toggles::default(),
            colors: Theme::default(),
            keys: Keymap::default(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Icons {
    #[default]
    Nerdfont,
    Unicode,
    None,
}

/// Startup defaults, used only for projects that have no saved state yet.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Toggles {
    pub show_hidden: bool,
    pub show_gitignored: bool,
    pub changed_only: bool,
}

impl Config {
    /// Loads the config, writing the documented defaults on first run.
    pub fn load() -> Result<Self> {
        let path = config_path()?;

        if !path.exists() {
            write_default(&path).with_context(|| format!("failed to create {}", path.display()))?;
        }

        let text = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;

        toml::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))
    }
}

fn write_default(path: &PathBuf) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(path, DEFAULT_CONFIG)?;

    Ok(())
}

pub fn state_dir() -> Result<PathBuf> {
    let strategy = choose_base_strategy().context("cannot locate the cache directory")?;

    Ok(strategy.cache_dir().join("bettertree/state"))
}

fn config_path() -> Result<PathBuf> {
    let strategy = choose_base_strategy().context("cannot locate the config directory")?;

    Ok(strategy.config_dir().join("bettertree/config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::Command;
    use crossterm::event::KeyEvent;
    use keys::KeyBinding;

    #[test]
    fn the_shipped_default_config_parses() {
        let config: Config = toml::from_str(DEFAULT_CONFIG).expect("default config is valid");

        assert_eq!(config.scroll_lines, 10);
        assert_eq!(config.icons, Icons::Nerdfont);
    }

    #[test]
    fn the_shipped_default_config_matches_the_hardcoded_defaults() {
        let shipped: Config = toml::from_str(DEFAULT_CONFIG).expect("default config is valid");
        let fallback = Config::default();

        assert_eq!(shipped.editor, fallback.editor);
        assert_eq!(shipped.scrolloff, fallback.scrolloff);
        assert_eq!(shipped.sort_order, fallback.sort_order);
        assert_eq!(shipped.show_diffstat, fallback.show_diffstat);
        assert_eq!(shipped.indent, fallback.indent);
        assert_eq!(shipped.watch, fallback.watch);

        let roles: [(&str, theme::Paint, theme::Paint); 5] = [
            (
                "directory",
                shipped.colors.directory,
                fallback.colors.directory,
            ),
            ("file", shipped.colors.file, fallback.colors.file),
            ("ignored", shipped.colors.ignored, fallback.colors.ignored),
            (
                "selection_bg",
                shipped.colors.selection_bg,
                fallback.colors.selection_bg,
            ),
            (
                "status_bar_bg",
                shipped.colors.status_bar_bg,
                fallback.colors.status_bar_bg,
            ),
        ];

        for (role, shipped, fallback) in roles {
            assert_eq!(shipped.0, fallback.0, "{role} drifted from the default");
        }
    }

    #[test]
    fn the_default_theme_uses_ansi_names_so_it_follows_the_terminal() {
        let config: Config = toml::from_str(DEFAULT_CONFIG).expect("default config is valid");

        assert_eq!(config.colors.directory.0, ratatui::style::Color::Blue);
        assert_eq!(config.colors.modified.0, ratatui::style::Color::Yellow);
        assert_eq!(config.colors.file.0, ratatui::style::Color::Reset);

        for line in DEFAULT_CONFIG.lines() {
            assert!(
                !line.contains(" = \"#"),
                "the shipped theme should not pin hex colours: {line}"
            );
        }
    }

    #[test]
    fn every_default_binding_resolves_to_a_command() {
        let config: Config = toml::from_str(DEFAULT_CONFIG).expect("default config is valid");
        let event = KeyEvent::from(crossterm::event::KeyCode::Char('q'));

        assert_eq!(config.keys.get(event), Some(Command::Quit));
    }

    #[test]
    fn commented_out_bindings_are_valid_too() {
        for line in DEFAULT_CONFIG.lines() {
            let Some(binding) = line.strip_prefix("# \"") else {
                continue;
            };
            let Some((spec, rest)) = binding.split_once("\" = \"") else {
                continue;
            };

            let name = rest.trim_end_matches('"');
            assert!(KeyBinding::parse(spec).is_ok(), "bad key: {spec}");
            assert!(Command::from_name(name).is_some(), "bad command: {name}");
        }
    }

    #[test]
    fn unknown_settings_are_rejected() {
        let error = toml::from_str::<Config>("nonsense = true").expect_err("should fail");

        assert!(error.to_string().contains("nonsense"), "{error}");
    }
}
