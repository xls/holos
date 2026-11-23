use std::sync::Arc;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::{Buffer, CellDiffOption};
use ratatui::style::Color;

use super::*;
use crate::config::{ColorDepth, Config, Keymap, Theme, ViewerMode};
use crate::viewer::ViewerId;
use crate::viewer::image::decode;
use crate::viewer::source::memory_opener;

/// A real PNG: the left half red, the right half pure green.
fn png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbaImage::from_fn(w, h, |x, _| {
        if x < w / 2 {
            image::Rgba([255, 0, 0, 255])
        } else {
            image::Rgba([0, 255, 0, 255])
        }
    });
    let mut out = Vec::new();
    image::ImageEncoder::write_image(
        image::codecs::png::PngEncoder::new(&mut out),
        img.as_raw(),
        w,
        h,
        image::ExtendedColorType::Rgba8,
    )
    .unwrap();
    out
}

/// An app with `shot.png` decoded and open full screen in mode 3.
fn app_with_picture(protocol: Protocol) -> App {
    let mut app = App::headless(Config::default(), Keymap::builtin(), Theme::blue());
    app.color_depth = ColorDepth::TrueColor;
    app.graphics = protocol;
    let bytes = Arc::new(png(40, 20));
    let len = bytes.len() as u64;
    let mut v = Viewer::open(
        ViewerId(1),
        "shot.png",
        None,
        memory_opener(bytes),
        Some(len),
        &app.config.viewer,
    )
    .unwrap();
    v.set_mode(ViewerMode::Render).unwrap();
    let job = v.take_image_job().unwrap();
    assert!(v.apply_image(&decode(job)));
    app.push_viewer(v);
    assert!(app.viewer_is_shown());
    app
}

/// One frame, prepared the way the event loop prepares it.
fn frame(app: &mut App, w: u16, h: u16) -> Buffer {
    let area = Rect::new(0, 0, w, h);
    let rows = crate::ui::viewer::body_rows(crate::ui::viewer::content_area(app, area));
    let cols = crate::ui::viewer::body_cols(app, area);
    app.service_viewer(rows, cols);
    if let Some((pw, ph)) = scaled_size(app, area) {
        app.viewer_mut().unwrap().prepare_scaled(pw, ph);
    }
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    // The frame's own buffer: a skipped cell never reaches the backend's.
    terminal
        .draw(|f| crate::ui::draw(f, app))
        .unwrap()
        .buffer
        .clone()
}

fn text(buf: &Buffer) -> String {
    let area = buf.area;
    (area.top()..area.bottom())
        .map(|y| {
            (area.left()..area.right())
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn half_blocks(buf: &Buffer) -> Vec<(u16, u16)> {
    let area = buf.area;
    (area.top()..area.bottom())
        .flat_map(|y| (area.left()..area.right()).map(move |x| (x, y)))
        .filter(|&(x, y)| buf[(x, y)].symbol() == "▀")
        .collect()
}

#[test]
fn half_blocks_draw_the_picture_in_its_own_colours() {
    let mut app = app_with_picture(Protocol::Halfblocks);
    let buf = frame(&mut app, 80, 24);
    let cells = half_blocks(&buf);
    assert!(cells.len() > 100, "{}", cells.len());
    let leftmost = cells.iter().min_by_key(|(x, _)| *x).unwrap();
    let rightmost = cells.iter().max_by_key(|(x, _)| *x).unwrap();
    assert_eq!(buf[*leftmost].fg, Color::Rgb(255, 0, 0));
    assert_eq!(buf[*leftmost].bg, Color::Rgb(255, 0, 0));
    assert_eq!(buf[*rightmost].fg, Color::Rgb(0, 255, 0));
}

#[test]
fn the_picture_keeps_its_proportion_and_is_centred() {
    let mut app = app_with_picture(Protocol::Halfblocks);
    let buf = frame(&mut app, 100, 24);
    let cells = half_blocks(&buf);
    let xs: Vec<u16> = cells.iter().map(|c| c.0).collect();
    let ys: Vec<u16> = cells.iter().map(|c| c.1).collect();
    let width = xs.iter().max().unwrap() - xs.iter().min().unwrap() + 1;
    let height = ys.iter().max().unwrap() - ys.iter().min().unwrap() + 1;
    // 40 x 20 pixels is 2:1, and a half-block row holds two pixels, so the
    // picture is four times as many columns as rows.
    assert_eq!(width, height * 4, "{width} x {height}");
    let left = *xs.iter().min().unwrap();
    let right = 100 - 1 - xs.iter().max().unwrap();
    assert!(left.abs_diff(right) <= 1, "centred: {left} / {right}");
}

#[test]
fn i_puts_the_template_facts_and_the_zoom_beside_the_picture() {
    let mut app = app_with_picture(Protocol::Halfblocks);
    app.viewer_mut().unwrap().toggle_inspect();
    let buf = frame(&mut app, 120, 30);
    let screen = text(&buf);
    assert!(screen.contains("PNG image"), "{screen}");
    assert!(screen.contains("40 x 20"), "{screen}");
    assert!(screen.contains("File size"), "{screen}");
    assert!(screen.contains("halfblocks"), "{screen}");
    // The picture is fitted beside the box, never under it.
    let boxed = (0..30_u16)
        .find_map(|y| {
            let row: String = (0..120_u16).map(|x| buf[(x, y)].symbol()).collect();
            row.find("File size").map(|at| at as u16)
        })
        .unwrap();
    assert!(half_blocks(&buf).iter().all(|(x, _)| *x < boxed - 2));
}

#[test]
fn a_sixel_picture_gets_a_placement_and_the_frame_leaves_its_cells_alone() {
    let mut app = app_with_picture(Protocol::Sixel);
    app.cell_px = (10, 20);
    let area = Rect::new(0, 0, 80, 24);
    let place = placement(&app, area).expect("a picture to send");
    assert_eq!(place.protocol, Protocol::Sixel);
    assert_eq!(place.viewer, 1);
    let buf = frame(&mut app, 80, 24);
    assert_eq!(
        buf[(place.x, place.y)].diff_option,
        CellDiffOption::Skip,
        "the frame must not paint over the picture"
    );
    assert!(half_blocks(&buf).is_empty());
    assert_eq!(
        app.viewer().unwrap().scaled_picture().unwrap().dimensions(),
        place.px
    );
}

#[test]
fn half_blocks_need_no_placement() {
    let app = app_with_picture(Protocol::Halfblocks);
    assert!(placement(&app, Rect::new(0, 0, 80, 24)).is_none());
}

#[test]
fn a_dialog_over_the_viewer_takes_the_picture_down() {
    let mut app = app_with_picture(Protocol::Kitty);
    let area = Rect::new(0, 0, 80, 24);
    assert!(placement(&app, area).is_some());
    app.push_dialog(Box::new(crate::dialog::MessageDialog::line("Copy", "done")));
    assert!(placement(&app, area).is_none());
}

#[test]
fn with_graphics_off_mode_three_shows_the_summary() {
    let mut app = app_with_picture(Protocol::Off);
    let buf = frame(&mut app, 80, 24);
    assert!(half_blocks(&buf).is_empty());
    assert!(text(&buf).contains("PNG image"));
}

#[test]
fn a_picture_still_decoding_says_so() {
    let mut app = App::headless(Config::default(), Keymap::builtin(), Theme::blue());
    let bytes = Arc::new(png(4, 4));
    let len = bytes.len() as u64;
    let mut v = Viewer::open(
        ViewerId(1),
        "shot.png",
        None,
        memory_opener(bytes),
        Some(len),
        &app.config.viewer,
    )
    .unwrap();
    v.set_mode(ViewerMode::Render).unwrap();
    let _job = v.take_image_job().unwrap();
    app.push_viewer(v);
    let buf = frame(&mut app, 80, 24);
    assert!(text(&buf).contains("decoding shot.png"), "{}", text(&buf));
}
