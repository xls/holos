use super::*;
use crate::config::{Config, Keymap, Theme};
use crate::ops::copy::tests::TempTree;
use crate::panel::Side;

fn app_at(left: &std::path::Path, right: &std::path::Path) -> App {
    let mut app = App::headless(Config::default(), Keymap::builtin(), Theme::blue());
    app.left.active_tab_mut().path = VfsPath::local(left);
    app.right.active_tab_mut().path = VfsPath::local(right);
    app
}

#[test]
fn the_same_directory_or_one_inside_the_other_is_refused() {
    let mut app = app_at(std::path::Path::new("/a"), std::path::Path::new("/a"));
    app.request_sync();
    assert!(app.sync_view().is_none());
    assert_eq!(
        app.message.as_deref(),
        Some("both panels show the same directory")
    );

    let mut app = app_at(std::path::Path::new("/a"), std::path::Path::new("/a/b"));
    app.request_sync();
    assert!(app.sync_view().is_none());
    assert!(
        app.message
            .as_deref()
            .unwrap_or_default()
            .contains("inside the other")
    );
}

#[test]
fn ctrl_alt_s_turns_the_panels_into_the_view_and_the_scan_fills_it() {
    use crate::input::{KeyCode, KeyEvent, KeyModifiers, dispatch};
    let left = TempTree::new("sync-app-l");
    let right = TempTree::new("sync-app-r");
    left.file("new.txt", b"new");
    left.file("both.txt", b"same");
    right.file("both.txt", b"same");
    let stamp = std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    for dir in [left.path(), right.path()] {
        std::fs::File::options()
            .write(true)
            .open(dir.join("both.txt"))
            .unwrap()
            .set_modified(stamp)
            .unwrap();
    }
    let mut app = app_at(left.path(), right.path());
    app.active_side = Side::Left;
    dispatch(
        &mut app,
        KeyEvent::new(
            KeyCode::Char('s'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        ),
    )
    .unwrap();
    assert!(app.sync_view().is_some());
    assert!(app.dialogs().is_empty(), "the panels, not a dialog");

    let rt = tokio::runtime::Runtime::new().unwrap();
    let _inside = rt.enter();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.service_sync();
        let done = app
            .sync_view()
            .is_some_and(|v| matches!(v.state(), crate::app::sync_view::ScanState::Done));
        if done {
            break;
        }
        assert!(Instant::now() < deadline, "the scan finished");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(app.sync_deadline().is_none(), "nothing left to wake for");
    let view = app.sync_view().unwrap();
    let rows: Vec<(String, sync::Status, sync::Plan)> = view
        .rows()
        .iter()
        .map(|r| (r.rel.clone(), r.status, r.plan))
        .collect();
    assert_eq!(
        rows,
        [
            ("both.txt".to_string(), sync::Status::Same, sync::Plan::Skip),
            (
                "new.txt".to_string(),
                sync::Status::OnlyLeft,
                sync::Plan::ToRight
            ),
        ]
    );

    app.run_sync();
    let jobs = app.take_pending_jobs();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].spec.kind, JobKind::Sync);
    assert_eq!(
        jobs[0].spec.targets,
        [VfsPath::local(right.path()).join("new.txt")]
    );
}

#[test]
fn leaving_the_view_stops_the_scan() {
    let mut app = app_at(std::path::Path::new("/a"), std::path::Path::new("/b"));
    app.request_sync();
    assert!(app.sync_deadline().is_some());
    app.leave_sync();
    app.service_sync();
    assert!(app.sync_deadline().is_none());
}

#[test]
fn a_read_only_target_is_refused_before_the_question() {
    let mut app = App::headless(Config::default(), Keymap::builtin(), Theme::blue());
    app.active_side = Side::Left;
    app.sync.view = Some(crate::app::sync_view::tests::view());
    app.right.active_tab_mut().caps.writable = false;
    app.ask_sync();
    assert!(app.dialogs().is_empty());
    assert!(
        app.message
            .as_deref()
            .unwrap_or_default()
            .contains("right side is read-only")
    );
    // Pointed the other way, the run only changes the left, which can be
    // written: unmark the two rows going right, point one row left.
    if let Some(view) = app.sync_view_mut() {
        view.move_to(0);
        view.toggle_mark();
        view.move_to(3);
        view.toggle_mark();
        view.move_to(2);
        view.point(false);
    }
    app.ask_sync();
    assert_eq!(app.dialogs().len(), 1);
}
