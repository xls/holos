use super::super::{Focus, KeyEvent, dispatch};
use super::*;
use crate::config::{Config, Keymap, Theme};
use crate::input::DialogId;
use crate::ops::JobKind;
use crate::ops::sync::Direction;
use crate::panel::Side;
use crate::vfs::VfsPath;

fn app() -> App {
    let mut app = App::headless(Config::default(), Keymap::builtin(), Theme::blue());
    app.focus = Focus::Panel(Side::Left);
    app.active_side = Side::Left;
    app.sync.view = Some(crate::app::sync_view::tests::view());
    app
}

fn press(app: &mut App, code: KeyCode, mods: KeyModifiers) {
    dispatch(app, KeyEvent::new(code, mods)).unwrap();
}

#[test]
fn space_unmarks_and_f5_asks_before_anything_runs() {
    let mut app = app();
    press(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);
    press(&mut app, KeyCode::F(5), KeyModifiers::NONE);
    assert_eq!(app.top_dialog().map(|d| d.id()), Some(DialogId::Sync));
    assert!(app.take_pending_jobs().is_empty(), "a question, not a job");

    app.pop_dialog();
    app.run_sync();
    let jobs = app.take_pending_jobs();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].spec.kind, JobKind::Sync);
    assert_eq!(
        jobs[0].spec.sources,
        [VfsPath::local("/src").join("zeta.md")],
        "the unmarked row stays behind"
    );
    let state = app.sync_view().map(|v| v.state().clone());
    assert!(
        matches!(
            &state,
            Some(crate::app::sync_view::ScanState::Running(progress))
                if progress.starts_with("synchronizing 0 of 1 files")
        ),
        "the view stays and says how far the run has got: {state:?}"
    );

    // When the job is done the trees are compared again, so the panels show
    // what is left to do.
    app.forget_job(jobs[0].id);
    app.service_sync();
    assert!(matches!(
        app.sync_view().map(|v| v.state().clone()),
        Some(crate::app::sync_view::ScanState::Scanning { .. })
    ));
    assert!(app.sync_deadline().is_some(), "a scan is queued");
}

#[test]
fn alt_m_and_shift_f5_step_the_mode_and_only_f5_asks() {
    use crate::app::sync_view::Mode;
    let mut app = app();
    press(&mut app, KeyCode::Char('m'), KeyModifiers::ALT);
    assert_eq!(app.sync_view().map(|v| v.mode()), Some(Mode::Both));
    press(&mut app, KeyCode::F(5), KeyModifiers::SHIFT);
    assert_eq!(app.sync_view().map(|v| v.mode()), Some(Mode::Mirror));
    assert!(app.dialogs().is_empty(), "a mode change asks nothing");
    press(&mut app, KeyCode::F(5), KeyModifiers::NONE);
    assert_eq!(app.top_dialog().map(|d| d.id()), Some(DialogId::Sync));
    assert_eq!(
        app.sync_view().map(|v| v.mode()),
        Some(Mode::Mirror),
        "F5 runs what is shown"
    );
}

#[test]
fn the_cursor_keys_move_one_cursor_and_the_arrows_point_the_row() {
    let mut app = app();
    press(&mut app, KeyCode::Down, KeyModifiers::NONE);
    press(&mut app, KeyCode::Down, KeyModifiers::NONE);
    press(&mut app, KeyCode::Right, KeyModifiers::NONE);
    let view = app.sync_view().unwrap();
    assert_eq!(view.current().map(|r| r.rel.as_str()), Some("photo.jpg"));
    assert_eq!(view.effective(2), crate::ops::sync::Plan::ToRight);
    assert!(
        matches!(app.focus, Focus::Panel(Side::Left)),
        "Right pointed the row rather than moving to the command line"
    );
}

#[test]
fn tab_moves_to_the_other_side_on_the_same_row() {
    let mut app = app();
    press(&mut app, KeyCode::Down, KeyModifiers::NONE);
    press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert_eq!(app.active_side, Side::Right);
    assert_eq!(
        app.sync_view().map(crate::app::sync_view::SyncView::cursor),
        Some(1)
    );
}

#[test]
fn alt_m_is_the_mode_not_the_mark_menu_and_esc_leaves() {
    let mut app = app();
    press(&mut app, KeyCode::Char('m'), KeyModifiers::ALT);
    assert!(app.dialogs().is_empty(), "not the Mark menu");
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.sync_view().is_none());
}

#[test]
fn typing_searches_and_esc_clears_the_search_before_leaving() {
    let mut app = app();
    press(&mut app, KeyCode::Char('z'), KeyModifiers::NONE);
    assert_eq!(
        app.sync_view()
            .and_then(|v| v.current())
            .map(|r| r.rel.as_str()),
        Some("zeta.md")
    );
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.sync_view().is_some());
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.sync_view().is_none());
}

#[test]
fn tab_makes_the_other_panel_the_source_of_mirror() {
    use crate::app::sync_view::Mode;
    use crate::ops::sync::Plan;
    let mut app = app();
    press(&mut app, KeyCode::Char('m'), KeyModifiers::ALT);
    press(&mut app, KeyCode::Char('m'), KeyModifiers::ALT);
    assert_eq!(app.sync_view().map(|v| v.mode()), Some(Mode::Mirror));
    // Mirror to the right: old.txt, only on the right, is trashed.
    assert_eq!(
        app.sync_view().map(|v| v.effective(1)),
        Some(Plan::DeleteRight)
    );

    press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    let view = app.sync_view().unwrap();
    assert_eq!(view.mode(), Mode::Mirror);
    assert_eq!(view.mode_label(true), "Mirror <-");
    // Now the left is made identical to the right: old.txt comes across, and
    // docs/new.md, only on the left, goes to the trash.
    assert_eq!(view.effective(1), Plan::ToLeft);
    assert_eq!(view.effective(0), Plan::DeleteLeft);

    // Both ways has no source: Tab changes nothing.
    press(&mut app, KeyCode::Char('m'), KeyModifiers::ALT);
    press(&mut app, KeyCode::Char('m'), KeyModifiers::ALT);
    assert_eq!(app.sync_view().map(|v| v.mode()), Some(Mode::Both));
    let before: Vec<Plan> = (0..5)
        .map(|i| app.sync_view().unwrap().effective(i))
        .collect();
    press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    let after: Vec<Plan> = (0..5)
        .map(|i| app.sync_view().unwrap().effective(i))
        .collect();
    assert_eq!(before, after);
}

#[test]
fn f3_shows_the_rows_two_copies_as_a_diff_and_a_one_sided_file_as_itself() {
    let mut app = app();
    press(&mut app, KeyCode::Down, KeyModifiers::NONE);
    press(&mut app, KeyCode::Down, KeyModifiers::NONE); // photo.jpg, on both sides
    press(&mut app, KeyCode::F(3), KeyModifiers::NONE);
    let Some(crate::app::ViewRequest::Diff { old, new }) = app.take_pending_view() else {
        panic!("a diff");
    };
    assert_eq!(old, VfsPath::local("/src").join("photo.jpg"));
    assert_eq!(new, VfsPath::local("/backup").join("photo.jpg"));

    press(&mut app, KeyCode::Up, KeyModifiers::NONE); // old.txt, right only
    press(&mut app, KeyCode::F(3), KeyModifiers::NONE);
    let Some(crate::app::ViewRequest::File { path, .. }) = app.take_pending_view() else {
        panic!("the one copy there is");
    };
    assert_eq!(path, VfsPath::local("/backup").join("old.txt"));
}

/// The owner's report, step by step: one changed file, already marked; Space
/// to "select" it unmarks it; F5. That asked anyway, ran an empty sync and
/// sat on "synchronizing 0 of 0 files" for good.
#[test]
fn f5_with_nothing_marked_says_so_and_runs_nothing() {
    let mut app = App::headless(Config::default(), Keymap::builtin(), Theme::blue());
    app.focus = Focus::Panel(Side::Left);
    app.active_side = Side::Left;
    let mut view = crate::app::sync_view::SyncView::new(
        VfsPath::local("/src"),
        VfsPath::local("/backup"),
        Direction::ToRight,
    );
    view.set_rows(
        vec![crate::ops::sync::Row {
            rel: "changed.txt".to_string(),
            left: Some(crate::ops::sync::Facts {
                size: 9,
                mtime: None,
            }),
            right: Some(crate::ops::sync::Facts {
                size: 3,
                mtime: None,
            }),
            status: crate::ops::sync::Status::LeftNewer,
            plan: crate::ops::sync::Plan::Skip,
        }],
        Vec::new(),
    );
    app.sync.view = Some(view);
    assert!(
        app.sync_view().unwrap().is_marked(0),
        "a changed file starts marked"
    );

    press(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);
    assert!(
        app.message
            .as_deref()
            .unwrap_or_default()
            .starts_with("unmarked"),
        "Space says what it did: {:?}",
        app.message
    );
    press(&mut app, KeyCode::F(5), KeyModifiers::NONE);
    assert!(app.dialogs().is_empty(), "nothing to ask about");
    assert!(
        app.message
            .as_deref()
            .unwrap_or_default()
            .contains("nothing is marked"),
        "{:?}",
        app.message
    );
    // And a run with nothing in it cannot start, whoever calls it.
    app.run_sync();
    assert!(app.take_pending_jobs().is_empty());
    assert!(matches!(
        app.sync_view().map(|v| v.state().clone()),
        Some(crate::app::sync_view::ScanState::Done)
    ));
}
