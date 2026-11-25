use super::*;
use crate::ops::sync::Facts;
use std::time::{Duration, UNIX_EPOCH};

fn facts(size: u64) -> Option<Facts> {
    Some(Facts {
        size,
        mtime: Some(UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
    })
}

fn row(rel: &str, left: Option<Facts>, right: Option<Facts>, status: Status) -> Row {
    Row {
        rel: rel.to_string(),
        left,
        right,
        status,
        plan: Plan::Skip,
    }
}

/// `/src` against `/backup`, one way to the right.
pub(crate) fn view() -> SyncView {
    let mut v = SyncView::new(
        VfsPath::local("/src"),
        VfsPath::local("/backup"),
        Direction::ToRight,
    );
    v.set_rows(
        vec![
            row("docs/new.md", facts(10), None, Status::OnlyLeft),
            row("old.txt", None, facts(20), Status::OnlyRight),
            row("photo.jpg", facts(30), facts(40), Status::RightNewer),
            row("same.txt", facts(5), facts(5), Status::Same),
            row("zeta.md", facts(7), facts(1), Status::LeftNewer),
        ],
        Vec::new(),
    );
    v.set_view_rows(10);
    v
}

#[test]
fn what_differs_starts_marked_and_the_same_files_start_hidden() {
    let v = view();
    let marked: Vec<bool> = (0..5).map(|i| v.is_marked(i)).collect();
    assert_eq!(marked, [true, false, false, false, true]);
    assert_eq!(v.shown(), [0, 1, 2, 4]);
    assert_eq!(v.summary(), "copy 2 to the right (17 B)");
}

#[test]
fn unmarking_a_row_takes_it_out_of_the_run_and_marking_puts_it_back() {
    let mut v = view();
    v.toggle_mark();
    assert_eq!(v.answer().copies.len(), 1, "only zeta.md is left");
    v.toggle_mark();
    assert_eq!(v.answer().copies.len(), 2);
    // A row with nothing planned has nothing to mark.
    v.move_to(1);
    v.toggle_mark();
    assert!(!v.is_marked(1));
}

#[test]
fn the_arrows_point_a_row_and_mark_it() {
    let mut v = view();
    v.move_to(2); // photo.jpg, newer on the right: skipped one way.
    v.point(true);
    assert_eq!(v.effective(2), Plan::ToRight);
    v.point(false);
    assert_eq!(v.effective(2), Plan::ToLeft);
    // Only on the right: Right makes the right match the left, by trashing.
    v.move_to(1);
    v.point(true);
    assert_eq!(v.effective(1), Plan::DeleteRight);
    assert_eq!(
        v.answer().deletes,
        [VfsPath::local("/backup").join("old.txt")]
    );
}

#[test]
fn both_ways_replans_and_marks_each_newer_side() {
    let mut v = view();
    v.plan_for(Direction::Both);
    let plans: Vec<Plan> = (0..5).map(|i| v.effective(i)).collect();
    assert_eq!(
        plans,
        [
            Plan::ToRight,
            Plan::ToLeft,
            Plan::ToLeft,
            Plan::Skip,
            Plan::ToRight
        ]
    );
}

#[test]
fn equals_shows_the_same_files_and_keeps_the_cursor_on_its_row() {
    let mut v = view();
    v.move_to(3); // zeta.md
    v.toggle_same();
    assert_eq!(v.current().map(|r| r.rel.as_str()), Some("zeta.md"));
    assert_eq!(v.shown().len(), 5);
}

#[test]
fn quick_search_finds_by_file_name() {
    let mut v = view();
    v.search_push('p');
    assert_eq!(v.current().map(|r| r.rel.as_str()), Some("photo.jpg"));
    // `new.md` is the name of `docs/new.md`.
    v.search_clear();
    v.search_push('n');
    assert_eq!(v.current().map(|r| r.rel.as_str()), Some("docs/new.md"));
}

#[test]
fn the_cursor_scrolls_the_rows_with_it() {
    let mut v = view();
    v.set_view_rows(2);
    v.move_to(3);
    assert_eq!(v.scroll(), 2);
    v.move_to(0);
    assert_eq!(v.scroll(), 0);
}

#[test]
fn the_mode_steps_one_way_both_ways_mirror_and_replans_each_time() {
    let mut v = view();
    assert_eq!(v.mode(), Mode::OneWay);
    v.cycle_mode();
    assert_eq!(v.mode(), Mode::Both);
    assert_eq!(v.effective(1), Plan::ToLeft);
    v.cycle_mode();
    assert_eq!(v.mode(), Mode::Mirror);
    assert_eq!(
        v.effective(1),
        Plan::DeleteRight,
        "mirror trashes what the source lacks"
    );
    assert_eq!(
        v.effective(2),
        Plan::ToRight,
        "and overwrites a newer target"
    );
    assert_eq!(v.mode_label(true), "Mirror ->");
    v.cycle_mode();
    assert_eq!(v.mode(), Mode::OneWay);
    assert_eq!(v.effective(1), Plan::Skip);
}
