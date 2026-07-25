use std::path::Path;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::git::DiffStat;

pub fn render(app: &App, frame: &mut Frame, area: Rect) {
    let base = Style::default()
        .fg(app.config.colors.ui_status_bar_fg.0)
        .bg(app.config.colors.ui_status_bar_bg.0);

    let left = Line::from(left_spans(app));
    let toggles = Line::from(toggle_spans(app));

    let left_width = left.width() as u16;
    let toggles_width = toggles.width() as u16;
    let fits = |line: &Line| centered(area.width, left_width, toggles_width, line.width() as u16);

    let full = Line::from(change_spans(app));
    let changes = match fits(&full).is_some() {
        true => full,
        false => Line::from(overall_diff_spans(app)),
    };

    frame.render_widget(Paragraph::new(left).style(base), area);

    render_at(frame, base, area, changes, |width| {
        centered(area.width, left_width, toggles_width, width)
    });

    render_at(frame, base, area, toggles, |width| {
        area.width.checked_sub(width)
    });
}

/// Places a segment at the offset the caller picks, dropping it when it would not fit.
fn render_at(
    frame: &mut Frame,
    base: Style,
    area: Rect,
    line: Line<'static>,
    offset: impl Fn(u16) -> Option<u16>,
) {
    let width = line.width() as u16;

    let Some(x) = offset(width) else {
        return;
    };

    let segment = Rect {
        x: area.x + x,
        width,
        ..area
    };

    frame.render_widget(Paragraph::new(line).style(base), segment);
}

/// Centres a segment in the bar, nudging it aside rather than overlapping its neighbours.
fn centered(total: u16, left: u16, right: u16, width: u16) -> Option<u16> {
    let free = total.checked_sub(left + right + width)?;

    Some(left + free / 2)
}

fn left_spans(app: &App) -> Vec<Span<'static>> {
    let colors = &app.config.colors;
    let mut spans = vec![Span::raw(format!(" {}", abbreviate_home(&app.root)))];

    if let Some(branch) = &app.git.branch {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!(" {branch}"),
            fg(colors.ui_accent.0).add_modifier(Modifier::BOLD),
        ));
    }

    if app.git_pending {
        spans.push(Span::styled("  git…", fg(colors.ui_muted_fg.0)));
    }

    spans
}

/// What survives when the bar is too narrow for the full breakdown.
fn overall_diff_spans(app: &App) -> Vec<Span<'static>> {
    if !app.git.is_repo() {
        return Vec::new();
    }

    let totals = &app.git.totals;
    let overall = DiffStat {
        added: totals.staged.added + totals.unstaged.added,
        removed: totals.staged.removed + totals.unstaged.removed,
    };

    diff_spans(app, &overall).into()
}

fn change_spans(app: &App) -> Vec<Span<'static>> {
    let mut spans = overall_diff_spans(app);

    if spans.is_empty() {
        return spans;
    }

    let colors = &app.config.colors;
    let totals = &app.git.totals;

    spans.push(Span::styled("   staged ", fg(colors.ui_muted_fg.0)));
    spans.extend(diff_spans(app, &totals.staged));

    spans.push(Span::styled("  unstaged ", fg(colors.ui_muted_fg.0)));
    spans.extend(diff_spans(app, &totals.unstaged));

    spans.push(Span::raw("   "));
    spans.push(Span::styled(
        format!("{}M", totals.modified_files),
        fg(colors.file_modified.0),
    ));
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        format!("{}S", totals.staged_files),
        fg(colors.file_staged.0),
    ));
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        format!("{}?", totals.untracked_files),
        fg(colors.file_untracked.0),
    ));

    spans
}

fn diff_spans(app: &App, stat: &DiffStat) -> [Span<'static>; 3] {
    [
        Span::styled(
            format!("+{}", stat.added),
            fg(app.config.colors.diff_added.0),
        ),
        Span::raw(" "),
        Span::styled(
            format!("-{}", stat.removed),
            fg(app.config.colors.diff_removed.0),
        ),
    ]
}

fn toggle_spans(app: &App) -> Vec<Span<'static>> {
    let colors = &app.config.colors;
    let indicator = |label: &'static str, active: bool| {
        let style = match active {
            true => fg(colors.ui_accent.0).add_modifier(Modifier::BOLD),
            false => fg(colors.ui_muted_fg.0).add_modifier(Modifier::DIM),
        };

        Span::styled(label, style)
    };

    vec![
        indicator("[h]", app.toggles.show_hidden),
        indicator("[i]", app.toggles.show_gitignored),
        indicator("[m]", app.toggles.changed_only),
        Span::raw(" "),
    ]
}

fn fg(color: Color) -> Style {
    Style::default().fg(color)
}

/// Paths under the home directory read better as `~/...`, the way a shell prompt shows them.
fn abbreviate_home(path: &Path) -> String {
    let Ok(home) = etcetera::home_dir() else {
        return path.display().to_string();
    };

    match path.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}
