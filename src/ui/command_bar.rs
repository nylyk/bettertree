use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use crate::command_line::{CommandLine, VISIBLE_CANDIDATES};
use crate::config::Config;

/// Draws the prompt on `prompt_area` and its completions in the bottom of `tree_area`.
pub fn render(
    line: &CommandLine,
    config: &Config,
    frame: &mut Frame,
    tree_area: Rect,
    prompt_area: Rect,
) {
    let prompt = Line::from(vec![
        Span::raw(":"),
        Span::raw(line.input().to_owned()),
        Span::styled("█", Style::default().fg(config.colors.directory.0)),
    ]);
    frame.render_widget(Paragraph::new(prompt), prompt_area);

    render_candidates(line, config, frame, tree_area);
}

fn render_candidates(line: &CommandLine, config: &Config, frame: &mut Frame, tree_area: Rect) {
    let visible = line.candidates().len().min(VISIBLE_CANDIDATES);
    if visible == 0 || tree_area.height == 0 {
        return;
    }

    let height = u16::try_from(visible)
        .unwrap_or(u16::MAX)
        .min(tree_area.height);
    let area = Rect {
        x: tree_area.x,
        y: tree_area.y + tree_area.height - height,
        width: tree_area.width,
        height,
    };

    let lines: Vec<Line> = line
        .candidates()
        .iter()
        .enumerate()
        .skip(line.scroll())
        .take(visible)
        .map(|(index, entry)| {
            let alias = entry
                .alias
                .map_or_else(String::new, |alias| format!(" ({alias})"));

            let row = Line::from(vec![
                Span::styled(
                    format!(" {}{alias}", entry.name),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  {}", entry.description),
                    Style::default().fg(config.colors.ignored.0),
                ),
            ]);

            match index == line.selected() {
                true => row.style(Style::default().bg(config.colors.selection_bg.0)),
                false => row,
            }
        })
        .collect();

    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines), area);
}
