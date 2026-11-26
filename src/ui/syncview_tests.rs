use super::*;
use crate::config::{ColorDepth, Config, Keymap, Theme};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn app(ascii: bool) -> App {
    let mut config = Config::default();
    config.ui.ascii_borders = ascii;
    let mut app = App::headless(config, Keymap::builtin(), Theme::blue());
    app.color_depth = ColorDepth::TrueColor;
    app.sync.view = Some(crate::app::sync_view::tests::view());
    app
}

fn screen(app: &mut App, w: u16, h: u16) -> Vec<String> {
    let area = Rect::new(0, 0, w, h);
    crate::ui::sync_view_rows(app, area);
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
    let buf = terminal.backend().buffer().clone();
    (0..h)
        .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>())
        .collect()
}

#[test]
fn each_file_faces_its_counterpart_with_the_sign_inside_each_panel() {
    let mut app = app(false);
    let lines = screen(&mut app, 100, 24);
    let row = |name: &str| {
        lines
            .iter()
            .find(|l| l.contains(name))
            .unwrap_or_else(|| panic!("{name} missing:\n{}", lines.join("\n")))
            .clone()
    };
    let cells =
        |line: &str, from: usize, n: usize| line.chars().skip(from).take(n).collect::<String>();
    // Columns 48 and 51 are the panels' own, against the borders at 49 and 50.
    let new = row("docs/new.md");
    assert_eq!(
        cells(&new, 48, 4),
        " \u{2502}\u{2502}\u{2192}",
        "the arrow only on the side that receives the file: {new}"
    );
    // Only on the right, unmarked one way: no sign, and the left half empty.
    let old = row("old.txt");
    assert_eq!(cells(&old, 48, 4), " \u{2502}\u{2502} ", "{old}");
    assert!(!cells(&old, 0, 49).contains("old.txt"));
    // Identical files start hidden; the mode is in the titles.
    let all = lines.join("\n");
    assert!(!all.contains("same.txt"));
    assert!(all.contains("copy 2 to the right"));
    assert!(lines[0].contains("One way \u{2192}"), "{}", lines[0]);
}

#[test]
fn changing_the_mode_changes_the_signs_before_anything_runs() {
    let mut app = app(false);
    if let Some(v) = app.sync_view_mut() {
        v.cycle_mode();
    }
    let lines = screen(&mut app, 100, 24);
    let old = lines.iter().find(|l| l.contains("old.txt")).unwrap();
    assert_eq!(
        old.chars().nth(51),
        Some(' '),
        "nothing on the side it comes from: {old}"
    );
    assert_eq!(
        old.chars().nth(48),
        Some('\u{2190}'),
        "against the border on the empty side too: {old}"
    );
    assert!(lines[0].contains("Both ways"), "{}", lines[0]);
    // And at the start of both status lines, so it is in view at the bottom
    // where the keys are listed.
    let status = lines.iter().rev().find(|l| l.contains("F5 sync")).unwrap();
    let halves: Vec<String> = [0, 50]
        .iter()
        .map(|from| status.chars().skip(from + 1).take(14).collect())
        .collect();
    for half in &halves {
        assert!(half.contains("Both ways"), "{status}");
    }
}

#[test]
fn it_draws_while_comparing_and_in_ascii_at_every_size() {
    for ascii in [false, true] {
        for (w, h) in [(200_u16, 50_u16), (80, 24), (60, 15)] {
            let mut app = app(ascii);
            let lines = screen(&mut app, w, h);
            if ascii {
                assert!(lines.iter().all(|l| l.is_ascii()), "{}", lines.join("\n"));
            }
            app.sync.view = Some(crate::app::sync_view::SyncView::new(
                crate::vfs::VfsPath::local("/a"),
                crate::vfs::VfsPath::local("/b"),
                crate::ops::sync::Direction::ToRight,
            ));
            if let Some(view) = app.sync_view_mut() {
                view.set_progress(42, 7);
            }
            let lines = screen(&mut app, w, h).join("\n");
            assert!(lines.contains("comparing... 42 files"), "{lines}");
            assert!(lines.contains("comparing... 7 files"), "{lines}");
        }
    }
}

#[test]
fn a_remote_side_is_titled_with_its_address_not_a_bare_path() {
    let mut app = app(false);
    if let Some(v) = app.sync_view_mut() {
        v.set_titles(
            "/home/thomas/TestData".to_string(),
            "sftp://thomas@nas.local:22/home/thomas/TestData".to_string(),
        );
    }
    let lines = screen(&mut app, 120, 20);
    assert!(lines[0].contains("sftp://thomas@nas"), "{}", lines[0]);
}

#[test]
fn the_byte_check_and_a_run_say_how_far_they_have_got() {
    let mut app = app(false);
    if let Some(v) = app.sync_view_mut() {
        v.set_checking(3, 40);
    }
    assert!(
        screen(&mut app, 100, 20)
            .join("\n")
            .contains("checking bytes: 3 of 40")
    );
    if let Some(v) = app.sync_view_mut() {
        v.set_running("synchronizing 2 of 5 files, 41.0M of 122.0M: big.iso".to_string());
    }
    assert!(
        screen(&mut app, 100, 20)
            .join("\n")
            .contains("2 of 5 files")
    );
}

#[test]
fn a_difference_left_unplanned_is_shown_and_explained() {
    let mut app = app(false);
    // photo.jpg is newer on the right, so one way leaves it; zeta.md is
    // unmarked by hand. Neither is a "nothing to do" while it is different.
    if let Some(v) = app.sync_view_mut() {
        v.move_to(0);
        v.toggle_mark();
        v.move_to(3);
        v.toggle_mark();
    }
    let all = screen(&mut app, 120, 20).join("\n");
    // zeta.md, unmarked, is the other: newer on the left.
    assert!(all.contains("nothing marked; 2 differ "), "{all}");
    let photo = all.lines().find(|l| l.contains("photo.jpg")).unwrap();
    assert!(photo.contains('\u{2260}'), "{photo}");
}

#[test]
fn the_view_says_it_is_sync_mode_and_how_to_leave_top_left() {
    let mut app = app(false);
    let lines = screen(&mut app, 100, 20);
    let top = &lines[0];
    assert!(top.starts_with("\u{250c} Sync mode - Esc to exit"), "{top}");
    assert_eq!(
        top.matches("Sync mode").count(),
        1,
        "once, on the left: {top}"
    );
}
