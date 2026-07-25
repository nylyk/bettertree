mod command_bar;
mod help;
mod icons;
mod status_bar;
mod tree_view;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::Style;
use ratatui::widgets::Paragraph;

use crate::app::{App, Mode};

pub fn render(app: &App, frame: &mut Frame) {
    let [status_area, tree_area, prompt_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    status_bar::render(app, frame, status_area);

    tree_view::render(app, frame, tree_area);

    match app.mode {
        Mode::Normal => render_message(app, frame, prompt_area),
        Mode::Command => command_bar::render(
            &app.command_line,
            &app.config,
            frame,
            tree_area,
            prompt_area,
        ),
        Mode::Help => {
            help::render(&app.config, app.help_scroll, frame, tree_area);
            render_message(app, frame, prompt_area);
        }
    }
}

fn render_message(app: &App, frame: &mut Frame, area: ratatui::layout::Rect) {
    let message = app.message.clone().unwrap_or_default();

    frame.render_widget(
        Paragraph::new(format!(" {message}"))
            .style(Style::default().fg(app.config.colors.ui_muted_fg.0)),
        area,
    );
}
