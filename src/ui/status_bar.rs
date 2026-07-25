use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::git::DiffStat;

pub fn render(app: &App, frame: &mut Frame, area: Rect) {
    let base = Style::default()
        .fg(app.config.colors.ui_status_bar_fg.0)
        .bg(app.config.colors.ui_status_bar_bg.0);

    let mut spans = vec![Span::raw(format!(" {}", app.root.display()))];

    if app.git_pending {
        spans.push(Span::raw("   git…"));
    }

    if app.git.is_repo() {
        if let Some(branch) = &app.git.branch {
            spans.push(Span::raw(format!("   {branch}")));
        }

        let totals = &app.git.totals;
        let overall = DiffStat {
            added: totals.staged.added + totals.unstaged.added,
            removed: totals.staged.removed + totals.unstaged.removed,
        };

        spans.push(Span::raw(format!(
            "   +{} -{}",
            overall.added, overall.removed
        )));
        spans.push(Span::raw(format!(
            "   staged +{} -{}",
            totals.staged.added, totals.staged.removed
        )));
        spans.push(Span::raw(format!(
            "  unstaged +{} -{}",
            totals.unstaged.added, totals.unstaged.removed
        )));
        spans.push(Span::raw(format!(
            "   {}M {}S {}?",
            totals.modified_files, totals.staged_files, totals.untracked_files
        )));
    }

    spans.push(Span::raw("  "));
    spans.push(indicator("[h]", app.toggles.show_hidden));
    spans.push(indicator("[i]", app.toggles.show_gitignored));
    spans.push(indicator("[m]", app.toggles.changed_only));

    frame.render_widget(Paragraph::new(Line::from(spans)).style(base), area);
}

fn indicator(label: &'static str, active: bool) -> Span<'static> {
    let style = match active {
        true => Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED),
        false => Style::default().add_modifier(Modifier::DIM),
    };

    Span::styled(label, style)
}
