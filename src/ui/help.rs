use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::commands::REGISTRY;
use crate::config::Config;

pub fn render(config: &Config, scroll: usize, frame: &mut Frame, area: Rect) {
    let key_width = REGISTRY
        .iter()
        .map(|entry| {
            config
                .keys
                .keys_for(entry.command)
                .join(" ")
                .chars()
                .count()
        })
        .max()
        .unwrap_or(0)
        .max(4);

    let name_width = REGISTRY
        .iter()
        .map(|entry| entry.name.len() + entry.alias.map_or(0, |alias| alias.len() + 3))
        .max()
        .unwrap_or(0);

    let mut lines = vec![Line::styled(
        format!(
            " {:key_width$}  {:name_width$}  {}",
            "keys", "command", "description"
        ),
        Style::default().add_modifier(Modifier::BOLD),
    )];

    lines.extend(REGISTRY.iter().skip(scroll).map(|entry| {
        let keys = config.keys.keys_for(entry.command).join(" ");
        let name = match entry.alias {
            Some(alias) => format!("{} ({alias})", entry.name),
            None => entry.name.to_owned(),
        };

        Line::from(vec![
            Span::styled(
                format!(" {keys:key_width$}  "),
                Style::default().fg(config.colors.staged.0),
            ),
            Span::raw(format!("{name:name_width$}  ")),
            Span::styled(
                entry.description.to_owned(),
                Style::default().fg(config.colors.ignored.0),
            ),
        ])
    }));

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" commands — `:` runs one by name, esc closes ");

    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}
