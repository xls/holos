//! A picture in the viewer, drawn.
//!
//! Two ways of drawing share one layout. Half blocks are characters, so they
//! are painted into the frame here like any other text. Kitty, sixel and
//! iTerm2 pictures are escape sequences sent after the frame, by the event
//! loop through [`crate::term::graphics_encode::Painter`]; for those this
//! module only keeps the place clear and says where the picture goes -
//! [`placement`] is the one answer both sides read, so the frame and the
//! picture cannot disagree about where it is.
//!
//! The picture is either the full-screen viewer's or the quick view's
//! (`Ctrl+Q`), whichever is on the screen; the quick view is drawn the same
//! way, inside its panel.
//!
//! A box of facts sits at the top right from the start - what the format's
//! template says, which is what mode 3 showed for an image before it drew
//! one - and `i` (the hex mode's reading panel key) hides it. The picture is
//! fitted beside the box rather than under it, because a kitty picture sits on
//! a layer above the text and would cover it.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::app::App;
use crate::config::theme::Rgb;
use crate::term::graphics::Protocol;
use crate::term::graphics_encode::Placement;
use crate::viewer::Viewer;
use crate::viewer::image::{Fit, ImageState, Picture, fit, half_block};

/// The pixel grid half blocks draw on: one column, two rows per cell.
const HALF_BLOCK_CELL: (u16, u16) = (1, 2);

/// The narrowest the picture is squeezed to make room for the box. Below this
/// the box is not drawn beside it.
const MIN_PICTURE_COLS: u16 = 16;

/// Whether this viewer's body is a picture rather than rows of text.
#[must_use]
pub fn shows_picture(app: &App, viewer: &Viewer) -> bool {
    app.graphics != Protocol::Off
        && viewer.is_picture()
        && matches!(
            viewer.image_state(),
            Some(ImageState::Pending | ImageState::Ready(_))
        )
}

/// The cell size a picture is fitted with under this session's protocol.
const fn cell_for(app: &App) -> (u16, u16) {
    if app.graphics.is_pixels() {
        app.cell_px
    } else {
        HALF_BLOCK_CELL
    }
}

/// The box of facts, and what is left for the picture.
fn split(app: &App, viewer: &Viewer, picture: &Picture, body: Rect) -> (Rect, Option<Rect>) {
    if !viewer.picture_facts() {
        return (body, None);
    }
    let lines = facts(app, viewer, picture, None);
    let widest = lines
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0)
        .saturating_add(4);
    let width = u16::try_from(widest)
        .unwrap_or(u16::MAX)
        .min(body.width / 2);
    let rows = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    let height = rows.saturating_add(2).min(body.height);
    if width < 12 || height < 3 || body.width.saturating_sub(width) < MIN_PICTURE_COLS {
        return (body, None);
    }
    let info = Rect {
        x: body.x.saturating_add(body.width.saturating_sub(width)),
        y: body.y,
        width,
        height,
    };
    let picture_area = Rect {
        width: body.width.saturating_sub(width).saturating_sub(1),
        ..body
    };
    (picture_area, Some(info))
}

/// Where the picture goes inside `body`, in cells, and at what pixel size.
fn layout(app: &App, viewer: &Viewer, body: Rect) -> Option<(Rect, Fit)> {
    let picture = viewer.picture()?;
    let (area, _) = split(app, viewer, picture, body);
    let fitted = fit(
        picture.width,
        picture.height,
        area.width,
        area.height,
        cell_for(app),
    )?;
    let cells = Rect {
        x: area.x.saturating_add(fitted.x),
        y: area.y.saturating_add(fitted.y),
        width: fitted.cols,
        height: fitted.rows,
    };
    Some((cells, fitted))
}

/// The full-screen viewer's body for a frame drawn in `area`.
fn viewer_body(app: &App, area: Rect) -> Rect {
    super::viewer::body_rect(super::viewer::content_area(app, area))
}

/// Which viewer's picture is on the screen.
///
/// At most one ever is: the full-screen viewer hides the panels, and so the
/// quick view in one of them, whenever it is up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Host {
    /// The `F3` viewer, full screen.
    Full,
    /// The quick view (`Ctrl+Q`), drawn in a panel.
    Quick,
}

/// The viewer whose picture this frame shows, and the body it is drawn in.
fn hosted(app: &App, area: Rect) -> Option<(Host, &Viewer, Rect)> {
    if super::viewer::is_backdrop(app) {
        let viewer = app.viewer()?;
        return shows_picture(app, viewer).then(|| (Host::Full, viewer, viewer_body(app, area)));
    }
    if super::console::is_backdrop(app) {
        return None;
    }
    let body = super::quick_view_body(app, area)?;
    let viewer = app.quick_viewer()?;
    shows_picture(app, viewer).then_some((Host::Quick, viewer, body))
}

/// The picture's pixel size this frame, and whose it is.
fn scaled_size(app: &App, area: Rect) -> Option<(Host, u32, u32)> {
    let (host, viewer, body) = hosted(app, area)?;
    let (_, fitted) = layout(app, viewer, body)?;
    Some((host, fitted.px_width, fitted.px_height))
}

/// Scale the picture on the screen to the size it is drawn at, before the
/// frame. Nothing is done when the size has not changed.
pub fn prepare(app: &mut App, area: Rect) {
    let Some((host, width, height)) = scaled_size(app, area) else {
        return;
    };
    let viewer = match host {
        Host::Full => app.viewer_mut(),
        Host::Quick => app.quick_viewer_mut(),
    };
    if let Some(viewer) = viewer {
        viewer.prepare_scaled(width, height);
    }
}

/// The scaled picture the painter sends after the frame.
#[must_use]
pub fn shown(app: &App, area: Rect) -> Option<&image::RgbaImage> {
    hosted(app, area).and_then(|(_, viewer, _)| viewer.scaled_picture())
}

/// Where a picture in real pixels goes this frame, or `None` for no picture.
///
/// None as well while anything is over the viewer: a dialog, the help page
/// on top of it. A kitty picture would sit above the dialog, and a sixel would
/// be drawn over it after the frame.
#[must_use]
pub fn placement(app: &App, area: Rect) -> Option<Placement> {
    if !app.graphics.is_pixels() || !app.dialogs().is_empty() {
        return None;
    }
    let (_, viewer, body) = hosted(app, area)?;
    let (cells, fitted) = layout(app, viewer, body)?;
    Some(Placement {
        viewer: viewer.id().0,
        x: cells.x,
        y: cells.y,
        cols: cells.width,
        rows: cells.height,
        px: (fitted.px_width, fitted.px_height),
        protocol: app.graphics,
    })
}

/// The viewer's background as the blend colour for transparent pixels.
pub fn background(app: &App) -> [u8; 3] {
    let Rgb { r, g, b } = app.theme.viewer.bg;
    [r, g, b]
}

/// Draw the body: the picture, or where it will be, and the facts box.
pub fn draw(f: &mut Frame, app: &App, viewer: &Viewer, body: Rect) {
    let style = Style::new()
        .fg(super::color(app, app.theme.viewer.fg))
        .bg(super::color(app, app.theme.viewer.bg));
    let Some(picture) = viewer.picture() else {
        // Asked for and not back yet.
        let note = format!("decoding {} ...", viewer.title());
        let row = Rect {
            y: body.y.saturating_add(body.height / 2),
            height: body.height.min(1),
            ..body
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(note, style))).centered(),
            row,
        );
        return;
    };
    let Some((cells, fitted)) = layout(app, viewer, body) else {
        return;
    };
    match app.graphics {
        Protocol::Halfblocks | Protocol::Blocks => paint_half_blocks(f, app, viewer, cells),
        // The picture is sent after the frame. Its cells are left alone by the
        // frame diff, or the text drawn into them would replace it.
        Protocol::Sixel | Protocol::Iterm2 => {
            if placement(app, f.area()).is_some() {
                skip(f, cells);
            }
        }
        Protocol::Kitty | Protocol::Off => {}
    }
    let (_, info) = split(app, viewer, picture, body);
    if let Some(info) = info {
        draw_facts(f, app, viewer, picture, info, fitted.percent);
    }
}

/// Mark cells as not the frame's to draw.
fn skip(f: &mut Frame, cells: Rect) {
    let buf = f.buffer_mut();
    for y in cells.top()..cells.bottom() {
        for x in cells.left()..cells.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_diff_option(ratatui::buffer::CellDiffOption::Skip);
            }
        }
    }
}

/// The picture as `▀` cells, top pixel in the foreground, bottom in the
/// background. Under `ui.ascii_borders`, and under the `blocks` style for a
/// terminal that draws `▀` short of the cell, a space in the two pixels'
/// average: a background always fills the whole cell.
fn paint_half_blocks(f: &mut Frame, app: &App, viewer: &Viewer, cells: Rect) {
    let Some(scaled) = viewer.scaled_picture() else {
        return;
    };
    let bg = background(app);
    // Coloured spaces under ASCII borders, where `▀` is not allowed, and under
    // `blocks`, for a terminal that draws `▀` short of the cell.
    let ascii = app.config.ui.ascii_borders || app.graphics == Protocol::Blocks;
    let colour = |[r, g, b]: [u8; 3]| app.theme.quantize(Rgb { r, g, b }, app.color_depth);
    let buf = f.buffer_mut();
    for row in 0..cells.height {
        for col in 0..cells.width {
            let (top, bottom) = half_block(scaled, u32::from(col), u32::from(row), bg);
            let Some(cell) =
                buf.cell_mut((cells.x.saturating_add(col), cells.y.saturating_add(row)))
            else {
                continue;
            };
            if ascii {
                let mix =
                    |a: u8, b: u8| u8::try_from((u16::from(a) + u16::from(b)) / 2).unwrap_or(a);
                let avg = [
                    mix(top[0], bottom[0]),
                    mix(top[1], bottom[1]),
                    mix(top[2], bottom[2]),
                ];
                cell.set_char(' ').set_bg(colour(avg));
            } else {
                cell.set_char('▀')
                    .set_fg(colour(top))
                    .set_bg(colour(bottom));
            }
        }
    }
}

/// The lines of the facts box.
///
/// What the format's template says where there is one - it reads the header
/// and knows the format's own words for things - and what the decoder found
/// where there is not. Then the two facts only this view has: the file's size
/// and how large the picture is shown.
fn facts(app: &App, viewer: &Viewer, picture: &Picture, percent: Option<u32>) -> Vec<String> {
    let template = viewer
        .rendered()
        .filter(|doc| doc.label != crate::viewer::image::FALLBACK_LABEL && !doc.lines.is_empty());
    let mut lines = match template {
        Some(doc) => doc.lines.iter().map(|l| l.text.clone()).collect(),
        None => vec![
            format!("{} image", picture.format),
            String::new(),
            format!("Dimensions   {} x {} px", picture.width, picture.height),
            format!("Colour       {}", picture.color),
        ],
    };
    lines.push(String::new());
    lines.push(format!(
        "File size    {}",
        crate::panel::format::human_size(viewer.len().unwrap_or(picture.bytes))
    ));
    // The same width whatever the zoom, so the box does not resize with the
    // window.
    lines.push(format!(
        "Shown at     {:>4}  {}",
        percent.map_or_else(|| "-".to_string(), |p| format!("{p}%")),
        app.graphics.label()
    ));
    lines
}

/// The facts box at `area`.
fn draw_facts(
    f: &mut Frame,
    app: &App,
    viewer: &Viewer,
    picture: &Picture,
    area: Rect,
    percent: u32,
) {
    let style = Style::new()
        .fg(super::color(app, app.theme.dialog.fg))
        .bg(super::color(app, app.theme.dialog.bg));
    let glyphs = super::text::Glyphs::new(app.config.ui.ascii_borders);
    let block = Block::bordered()
        .border_set(glyphs.border_set())
        .border_style(style)
        // `I` underlined, as a dialog marks its mnemonics: it is the key that
        // hides the box.
        .title(Line::from(vec![
            Span::styled(" ", style),
            Span::styled("I", style.add_modifier(Modifier::UNDERLINED)),
            Span::styled("nfo ", style),
        ]))
        .style(style);
    let inner = block.inner(area);
    f.render_widget(ratatui::widgets::Clear, area);
    f.render_widget(block, area);
    let lines = facts(app, viewer, picture, Some(percent));
    for (i, text) in lines.iter().enumerate() {
        let Ok(row) = u16::try_from(i) else { break };
        if row >= inner.height {
            break;
        }
        let line = super::text::fit_left(
            &format!(" {text}"),
            usize::from(inner.width),
            super::text::Crop::End,
            glyphs.ellipsis(),
        );
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(line, style))),
            Rect {
                y: inner.y.saturating_add(row),
                height: 1,
                ..inner
            },
        );
    }
}

#[cfg(test)]
#[path = "image_tests.rs"]
mod tests;
