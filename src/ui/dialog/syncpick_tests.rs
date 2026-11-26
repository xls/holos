use super::*;
use crate::config::{ColorDepth, Theme};
use crate::input::{KeyModifiers, KeyPress};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn key(code: KeyCode) -> DialogKey {
    DialogKey::raw(KeyPress::new(code, KeyModifiers::NONE))
}

fn choice(name: &str, left: &str, right: &str) -> Choice {
    Choice {
        name: name.to_string(),
        left: left.to_string(),
        right: right.to_string(),
    }
}

fn picker(saved: usize) -> SyncPicker {
    SyncPicker::new(
        choice("Compare these panels", "/a", "/b"),
        (0..saved)
            .map(|n| {
                choice(
                    &format!("pair {n}"),
                    &format!("/left/{n}"),
                    &format!("/right/{n}"),
                )
            })
            .collect(),
    )
}

fn screen(p: &mut SyncPicker, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    let style = DialogStyle::new(&Theme::blue(), ColorDepth::TrueColor, false);
    terminal
        .draw(|f| {
            let inner = crate::dialog::dialog_interior(f.area(), p);
            p.layout(inner);
            crate::dialog::draw(f, p, f.area(), &style);
        })
        .unwrap();
    let buf = terminal.backend().buffer().clone();
    (0..h)
        .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn enter_answers_the_panels_or_the_pair_under_the_cursor() {
    let mut p = picker(3);
    let DialogOutcome::Accept(DialogResult::Text(t)) = p.handle_key(&key(KeyCode::Enter)) else {
        panic!("Enter answers");
    };
    assert_eq!(read_answer(&t), Pick::Panels);
    p.handle_key(&key(KeyCode::Down));
    p.handle_key(&key(KeyCode::Down));
    let DialogOutcome::Accept(DialogResult::Text(t)) = p.handle_key(&key(KeyCode::Enter)) else {
        panic!("Enter answers");
    };
    assert_eq!(read_answer(&t), Pick::Open(1));
}

#[test]
fn twenty_pairs_scroll_and_the_cursor_stays_in_view() {
    let mut p = picker(20);
    screen(&mut p, 120, 24);
    p.handle_key(&key(KeyCode::End));
    let out = screen(&mut p, 120, 24);
    assert!(out.contains("pair 19"), "the last pair is in view:\n{out}");
    assert!(
        !out.contains("pair 0 "),
        "the first has scrolled off:\n{out}"
    );
}

#[test]
fn typing_filters_by_name_or_path_and_esc_clears_before_closing() {
    let mut p = picker(20);
    for c in "17".chars() {
        p.handle_key(&key(KeyCode::Char(c)));
    }
    assert_eq!(p.shown(), [None, Some(17)], "the panels stay first");
    let DialogOutcome::Accept(DialogResult::Text(t)) = p.handle_key(&key(KeyCode::Enter)) else {
        panic!("Enter answers");
    };
    assert_eq!(read_answer(&t), Pick::Open(17), "on the match");
    let mut p = picker(3);
    p.handle_key(&key(KeyCode::Char('x')));
    assert!(matches!(
        p.handle_key(&key(KeyCode::Esc)),
        DialogOutcome::Consumed
    ));
    assert_eq!(p.search(), "");
    assert!(matches!(
        p.handle_key(&key(KeyCode::Esc)),
        DialogOutcome::Cancel
    ));
}

#[test]
fn f2_asks_to_rename_and_del_forgets_but_not_the_panels() {
    let mut p = picker(2);
    assert!(matches!(
        p.handle_key(&key(KeyCode::F(2))),
        DialogOutcome::Consumed
    ));
    p.handle_key(&key(KeyCode::Down));
    let DialogOutcome::Act(DialogResult::Text(t)) = p.handle_key(&key(KeyCode::F(2))) else {
        panic!("F2 acts and stays open");
    };
    assert_eq!(read_answer(&t), Pick::Rename(0));
    let DialogOutcome::Act(DialogResult::Text(t)) = p.handle_key(&key(KeyCode::Delete)) else {
        panic!("Del acts and stays open");
    };
    assert_eq!(read_answer(&t), Pick::Forget(0));
    assert_eq!(p.saved().len(), 1);
}

#[test]
fn a_long_path_keeps_both_its_ends() {
    let long = format!(
        "/home/thomas/{}/projects/holoscommander",
        "very-deep/".repeat(12)
    );
    let mut p = SyncPicker::new(
        choice(
            "Compare these panels",
            &long,
            "sftp://thomas@nas.local:22/backup/holoscommander",
        ),
        Vec::new(),
    );
    let out = screen(&mut p, 100, 12);
    assert!(
        out.contains("/home/thomas"),
        "the start of the left path:\n{out}"
    );
    assert!(out.contains("holoscommander"), "and its end:\n{out}");
    assert!(
        out.contains("sftp://"),
        "the start of the right one:\n{out}"
    );
}
