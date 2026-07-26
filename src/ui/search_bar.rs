use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::config::Config;
use crate::search::Search;

/// Draws the `/` prompt, with the number of matches at the far end. The results are the tree
/// itself, narrowed down, so nothing else has to be drawn here.
pub fn render(search: &Search, matches: usize, config: &Config, frame: &mut Frame, area: Rect) {
    let prompt = format!("/{}█", search.input());
    let count = format!("{matches} matches ");

    let used = prompt.chars().count() + count.chars().count();
    let gap = usize::from(area.width).saturating_sub(used);

    let line = Line::from(vec![
        Span::raw(prompt),
        Span::raw(" ".repeat(gap)),
        Span::styled(count, Style::default().fg(config.colors.ui_muted_fg.0)),
    ]);

    frame.render_widget(Paragraph::new(line), area);
}
