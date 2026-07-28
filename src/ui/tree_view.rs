use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::icons;
use crate::app::App;
use crate::config::Config;
use crate::git::{DiffStat, GitInfo};
use crate::tree::{Node, RowKind};

pub fn render(app: &App, frame: &mut Frame, area: Rect) {
    let height = usize::from(area.height);
    let width = usize::from(area.width);

    let lines: Vec<Line> = app
        .tree
        .rows()
        .iter()
        .enumerate()
        .skip(app.scroll)
        .take(height)
        .map(|(index, row)| match row.kind {
            RowKind::Entry(id) => line(
                app.tree.node(id),
                Layout {
                    depth: row.depth,
                    open: app.tree.is_open(id),
                    selected: app.is_selected(index),
                    width,
                },
                &app.config,
                &app.git,
            ),
            RowKind::More(cut) => more_line(cut, row.depth, &app.config),
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), area);
}

/// Stands in for the entries the display cap left out of a folder.
fn more_line(cut: usize, depth: usize, config: &Config) -> Line<'static> {
    let indent = " ".repeat(depth * config.indent + 1);

    Line::from(Span::styled(
        format!("{indent}… {cut} more entries …"),
        Style::default().fg(config.colors.ui_muted_fg.0),
    ))
}

/// Where a row sits on screen, as opposed to what it contains.
struct Layout {
    depth: usize,
    open: bool,
    selected: bool,
    width: usize,
}

fn line(node: &Node, layout: Layout, config: &Config, git: &GitInfo) -> Line<'static> {
    let (marker, glyph) = icons::prefix(node, layout.open, config.icons);

    let mut left = vec![
        Span::raw(" ".repeat(layout.depth * config.indent + 1)),
        Span::raw(marker),
        Span::raw(glyph),
        Span::styled(node.name.clone(), name_style(node, config, git)),
    ];

    if layout.open && !node.is_loaded() {
        left.push(Span::styled(
            " …",
            Style::default().fg(config.colors.ui_muted_fg.0),
        ));
    }

    let right = decorations(node, layout.open, config, git);

    let used: usize = left.iter().chain(right.iter()).map(Span::width).sum();
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(layout.width.saturating_sub(used))));
    spans.extend(right);

    let line = Line::from(spans);

    match layout.selected {
        true => line.style(Style::default().bg(config.colors.ui_selection_bg.0)),
        false => line,
    }
}

fn name_style(node: &Node, config: &Config, git: &GitInfo) -> Style {
    let colors = &config.colors;

    let color = match git.file(&node.path) {
        Some(status) if status.conflicted => colors.file_conflicted.0,
        Some(status) if status.untracked => colors.file_untracked.0,
        Some(status) if status.unstaged.is_some() => colors.file_modified.0,
        Some(_) => colors.file_staged.0,
        None if node.kind.is_dir() && git.directory(&node.path).is_some() => colors.dir_changed.0,
        None if node.ignored == Some(true) => colors.file_ignored.0,
        None if node.kind.is_dir() => colors.dir.0,
        None => colors.file.0,
    };

    let style = Style::default().fg(color);

    match node.symlink {
        true => style.add_modifier(Modifier::ITALIC),
        false => style,
    }
}

/// The right-hand columns: a diff stat, then the two-letter status code.
fn decorations(node: &Node, open: bool, config: &Config, git: &GitInfo) -> Vec<Span<'static>> {
    let status = git.file(&node.path);

    // An expanded folder says nothing of its own; its visible children carry the detail.
    let stat = match status {
        Some(status) => status.stat(),
        None if open => DiffStat::default(),
        None => git.directory(&node.path).unwrap_or_default(),
    };

    let mut spans = Vec::new();

    if config.show_diffstat && !stat.is_empty() {
        spans.push(Span::styled(
            format!(" +{}", stat.added),
            Style::default().fg(config.colors.diff_added.0),
        ));
        spans.push(Span::styled(
            format!(" -{}", stat.removed),
            Style::default().fg(config.colors.diff_removed.0),
        ));
    }

    let code = status.map_or_else(|| "  ".to_owned(), |status| status.code());
    spans.push(Span::styled(
        format!("  {code} "),
        Style::default().fg(code_color(node, config, git)),
    ));

    spans
}

fn code_color(node: &Node, config: &Config, git: &GitInfo) -> Color {
    match git.file(&node.path) {
        Some(status) if status.conflicted => config.colors.file_conflicted.0,
        Some(status) if status.untracked => config.colors.file_untracked.0,
        Some(status) if status.unstaged.is_some() => config.colors.file_modified.0,
        Some(_) => config.colors.file_staged.0,
        None => config.colors.file.0,
    }
}
