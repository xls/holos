//! The sync view, drawn in the two panels.
//!
//! ```text
//! ┌ ~/projects ─── One way → ┐┌ /mnt/nas/projects ─ One way → ┐
//! │Name          Size Date   ││  Name          Size Date      │
//! │manual.pdf    2.1M 12 Nov  ││→ manual.pdf    1.9M 02 Mar     │
//! │                         ←││  img_031.jpg   4.8M 20 Nov     │
//! │config.ini    3.0K 05 Nov ≠││≠ config.ini    3.1K 05 Nov     │
//! ├──────────────────────────┤├───────────────────────────────┤
//! │copy 2 (6.9 M)            ││F5 sync  Alt+M mode  Space mark │
//! └──────────────────────────┘└───────────────────────────────┘
//! ```
//!
//! Each panel draws its own side of every row, so a file always faces its
//! counterpart, and each has one column against the middle border saying
//! what will be done to the row ([`sign`]) - inside the panel, so the borders
//! stay borders. Both panels are laid out without a tab bar, so their rows
//! line up whatever tabs either has. The mode is in both titles.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::panelview::{CursorStyle, rows};
use super::text::{self, Crop, Glyphs};
use crate::app::App;
use crate::app::sync_view::{ScanState, SyncView};
use crate::config::theme::Rgb;
use crate::ops::sync::{Facts, Plan, Status};
use crate::panel::Side;
use crate::panel::format::{human_size, time_text};

/// What the left panel's top border starts with while the view is up.
pub const BADGE: &str = " Sync mode - Esc to exit ";

/// The widest a date is drawn.
const DATE_MAX: usize = 16;

/// The sign in `side`'s border column for one row: what the run will do to
/// that side - shown only on the side that changes - or, for a row it leaves
/// alone, how the two sides compare, which is about both.
#[must_use]
pub fn sign(view: &SyncView, index: usize, side: Side, ascii: bool) -> char {
    let Some(row) = view.rows().get(index) else {
        return ' ';
    };
    let pick = |fancy: char, plain: char| if ascii { plain } else { fancy };
    // Only on the side that changes: the target of a copy, the side a file
    // is trashed from. The other side is not touched, so it says nothing.
    match (view.effective(index), side) {
        (Plan::ToRight, Side::Right) => pick('\u{2192}', '>'),
        (Plan::ToLeft, Side::Left) => pick('\u{2190}', '<'),
        (Plan::DeleteLeft, Side::Left) | (Plan::DeleteRight, Side::Right) => pick('\u{d7}', 'x'),
        (Plan::ToRight | Plan::DeleteRight, Side::Left)
        | (Plan::ToLeft | Plan::DeleteLeft, Side::Right) => ' ',
        (Plan::Skip, _) => match row.status {
            Status::Same => '=',
            Status::Differs => pick('\u{2260}', '!'),
            Status::Clash => '!',
            // Different, and left alone: one way does not overwrite a newer
            // target. Said, so a difference is never a blank row.
            Status::LeftNewer | Status::RightNewer => pick('\u{2260}', '!'),
            Status::OnlyLeft | Status::OnlyRight => ' ',
        },
    }
}

/// Draw one panel's side of the sync view.
pub fn draw_panel(f: &mut Frame, app: &App, side: Side, area: Rect) {
    let Some(view) = app.sync_view() else {
        return;
    };
    if area.width < 4 || area.height < 4 {
        return;
    }
    let g = Glyphs::new(app.config.ui.ascii_borders);
    let theme = &app.theme;
    let color = |rgb: Rgb| theme.quantize(rgb, app.color_depth);
    let border = if app.active_side == side {
        theme.panel.border
    } else {
        theme.panel.inactive_border
    };
    let inner = usize::from(area.width).saturating_sub(2);
    // Top left of the screen, in the cursor's colours so it cannot be missed:
    // the panels are not showing directories, and Esc is the way back.
    let badge = if side == Side::Left { BADGE } else { "" };
    let mode = format!(" {} ", view.mode_label(app.config.ui.ascii_borders));
    let title = text::fit_left(
        &view.title(side == Side::Left),
        inner
            .saturating_sub(text::width(&mode) + 4)
            .saturating_sub(text::width(badge)),
        Crop::Middle,
        g.ellipsis(),
    );
    let mut heading = Vec::new();
    if !badge.is_empty() {
        let (bg, fg) = CursorStyle::Focused.colors(theme);
        heading.push(Span::styled(
            badge,
            Style::new()
                .fg(color(fg))
                .bg(color(bg))
                .add_modifier(Modifier::BOLD),
        ));
    }
    heading.push(Span::styled(
        format!(" {title} "),
        Style::new().fg(color(theme.panel.header_fg)),
    ));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(g.border_set())
        .border_style(Style::new().fg(color(border)))
        .title_top(Line::from(heading))
        .title_top(
            Line::from(Span::styled(
                mode,
                Style::new().fg(color(theme.panel.header_fg)),
            ))
            .right_aligned(),
        )
        .style(
            Style::new()
                .bg(color(theme.panel.bg))
                .fg(color(theme.panel.fg)),
        );
    f.render_widget(block, area);

    let r = rows(area, false);
    let body = Style::new()
        .fg(color(theme.panel.fg))
        .bg(color(theme.panel.bg));
    let header = Style::new()
        .fg(color(theme.panel.header_fg))
        .bg(color(theme.panel.header_bg));
    let width = usize::from(r.entries.width);
    let date_w = date_width(app);
    put(
        f,
        r.header,
        beside(
            side,
            ' ',
            &line_text("Name", "Size", "Date", width.saturating_sub(2), date_w),
        ),
        header,
    );

    match view.state() {
        ScanState::Scanning { left, right } => {
            let count = match side {
                Side::Left => left,
                Side::Right => right,
            };
            centred(f, r.entries, &format!("comparing... {count} files"), body);
        }
        ScanState::Failed(why) => centred(f, r.entries, why, body),
        ScanState::Running(progress) => centred(f, r.entries, progress, body),
        ScanState::Checking { done, total } => centred(
            f,
            r.entries,
            &format!("checking bytes: {done} of {total} files"),
            body,
        ),
        ScanState::Done => draw_rows(f, app, view, side, r.entries, date_w),
    }

    draw_rule(f, app, r.rule, g, border);
    let status = Style::new()
        .fg(color(theme.panel.status_fg))
        .bg(color(theme.panel.status_bg));
    // The mode leads both status lines, in the header colour so it stands out
    // from what follows: it is what F5 will do, and Alt+M changes it.
    let ascii = app.config.ui.ascii_borders;
    let mode = Style::new()
        .fg(color(theme.panel.header_fg))
        .bg(color(theme.panel.status_bg))
        .add_modifier(Modifier::BOLD);
    put_mode(
        f,
        r.status,
        &format!(" {} ", view.mode_label(ascii)),
        mode,
        &status_text(view, side, ascii),
        status,
    );
}

fn draw_rows(f: &mut Frame, app: &App, view: &SyncView, side: Side, area: Rect, date_w: usize) {
    let theme = &app.theme;
    let color = |rgb: Rgb| theme.quantize(rgb, app.color_depth);
    let body = Style::new()
        .fg(color(theme.panel.fg))
        .bg(color(theme.panel.bg));
    let marked = Style::new()
        .fg(color(theme.panel.marked_fg))
        .bg(color(theme.panel.bg));
    let (cursor_bg, cursor_fg) = CursorStyle::of(app, side).colors(theme);
    let width = usize::from(area.width);
    let shown = view.shown();
    if shown.is_empty() {
        let note = if view.show_same() {
            "the two trees are the same"
        } else {
            "nothing differs (= shows identical files)"
        };
        centred(f, area, note, body);
        return;
    }
    for offset in 0..area.height {
        let at = view.scroll().saturating_add(usize::from(offset));
        let Some(&index) = shown.get(at) else {
            break;
        };
        let Some(row) = view.rows().get(index) else {
            continue;
        };
        let facts = match side {
            Side::Left => row.left,
            Side::Right => row.right,
        };
        // A side without the file is blank, the width of a row, so the sign
        // still lands against the border.
        let text = facts.map_or_else(
            || " ".repeat(width.saturating_sub(2)),
            |facts| {
                row_text(
                    &row.rel,
                    facts,
                    width.saturating_sub(2),
                    date_w,
                    &app.config.panel.date_format,
                )
            },
        );
        let text = beside(
            side,
            sign(view, index, side, app.config.ui.ascii_borders),
            &text,
        );
        let mut style = if view.effective(index) == Plan::Skip {
            body
        } else {
            marked
        };
        if at == view.cursor() {
            style = Style::new().fg(color(cursor_fg)).bg(color(cursor_bg));
        }
        let rect = Rect {
            y: area.y.saturating_add(offset),
            height: 1,
            ..area
        };
        put(f, rect, format!("{text:<width$}"), style);
    }
}

/// `text` with the sign column on the side that faces the other panel, and
/// a space between them.
fn beside(side: Side, sign: char, text: &str) -> String {
    match side {
        Side::Left => format!("{text} {sign}"),
        Side::Right => format!("{sign} {text}"),
    }
}

fn row_text(rel: &str, facts: Facts, width: usize, date_w: usize, format: &str) -> String {
    let date = text::truncate(&time_text(facts.mtime, format), date_w, Crop::End, "~");
    line_text(rel, &human_size(facts.size), &date, width, date_w)
}

/// `name`, then the size and the date right-aligned in fixed columns.
fn line_text(name: &str, size: &str, date: &str, width: usize, date_w: usize) -> String {
    let tail = format!(" {size:>6} {date:<date_w$}");
    let tail_w = text::width(&tail);
    if width <= tail_w + 4 {
        return text::fit_left(name, width, Crop::Middle, "~");
    }
    let room = width - tail_w;
    let name = text::fit_left(name, room, Crop::Middle, "~");
    let name = format!("{name:<room$}");
    format!("{name}{tail}")
}

fn date_width(app: &App) -> usize {
    text::width(&time_text(
        Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000)),
        &app.config.panel.date_format,
    ))
    .clamp(4, DATE_MAX)
}

fn status_text(view: &SyncView, side: Side, ascii: bool) -> String {
    match side {
        Side::Left => {
            let mut line = match view.state() {
                ScanState::Done => view.summary(),
                ScanState::Scanning { .. } => "comparing...".to_string(),
                ScanState::Failed(_) => "could not compare".to_string(),
                ScanState::Running(progress) => progress.clone(),
                ScanState::Checking { done, total } => {
                    format!("checking bytes: {done} of {total}")
                }
            };
            if view.contents() {
                line.push_str("  [bytes checked]");
            }
            if !view.unreadable().is_empty() {
                line.push_str(&format!("  {} unreadable", view.unreadable().len()));
            }
            line
        }
        Side::Right if !view.search().is_empty() => format!("search: {}", view.search()),
        Side::Right => {
            let arrows = if ascii { "<- ->" } else { "\u{2190}\u{2192}" };
            let check = if view.contents() {
                "Alt+C stop checking bytes"
            } else {
                "Alt+C check bytes"
            };
            format!("F5 sync  Alt+M mode  Space mark  {arrows} direction  = same  {check}")
        }
    }
}

/// A status line: the mode in its own style, then `text`.
fn put_mode(f: &mut Frame, area: Rect, label: &str, label_style: Style, text: &str, style: Style) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let width = usize::from(area.width);
    // Truncated, not fitted: fitting pads to the width, and the rest of the
    // line would have no room at all.
    let label = text::truncate(label, width, Crop::End, "~");
    let rest = text::fit_left(
        &format!(" {text}"),
        width.saturating_sub(text::width(&label)),
        Crop::End,
        "~",
    );
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(label, label_style),
            Span::styled(rest, style),
        ]))
        .style(style),
        area,
    );
}

fn put(f: &mut Frame, area: Rect, text: String, style: Style) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let text = text::fit_left(&text, usize::from(area.width), Crop::End, "~");
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(text, style))).style(style),
        area,
    );
}

fn centred(f: &mut Frame, area: Rect, message: &str, style: Style) {
    if area.height == 0 {
        return;
    }
    let row = Rect {
        y: area.y.saturating_add(area.height / 2),
        height: 1,
        ..area
    };
    let text = text::fit_left(message, usize::from(area.width), Crop::End, "~");
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(text, style))).centered(),
        row,
    );
}

fn draw_rule(f: &mut Frame, app: &App, area: Rect, g: Glyphs, border: Rgb) {
    if area.width < 2 || area.height == 0 {
        return;
    }
    let theme = &app.theme;
    let color = |rgb: Rgb| theme.quantize(rgb, app.color_depth);
    let mid = usize::from(area.width).saturating_sub(2);
    let line = format!(
        "{}{}{}",
        g.tee_left(),
        g.horizontal().repeat(mid),
        g.tee_right()
    );
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            line,
            Style::new().fg(color(border)).bg(color(theme.panel.bg)),
        ))),
        area,
    );
}

#[cfg(test)]
#[path = "syncview_tests.rs"]
mod tests;
