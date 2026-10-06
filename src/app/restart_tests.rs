//! The watch on the binary, and the restart question it leads to.

use super::*;
use crate::config::{Config, Keymap, Theme};

/// A stand-in binary in a scratch directory.
fn binary(tag: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("hcmd-restart-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("hcmd");
    std::fs::write(&path, b"old binary").expect("write");
    (dir, path)
}

/// Replace `path` the way the installer does: write beside it, rename over.
fn install_over(path: &Path) {
    let pending = path.with_extension("pending");
    std::fs::write(&pending, b"the new binary, longer").expect("write");
    std::fs::rename(&pending, path).expect("rename");
}

#[test]
fn an_untouched_binary_is_not_a_replacement_and_a_renamed_one_is() {
    let (dir, path) = binary("watch");
    let start = Instant::now();
    let mut watch = ExeWatch::arm(path.clone(), start).expect("armed");
    let later = start + POLL;
    assert!(!watch.replaced(later), "nothing changed");
    install_over(&path);
    assert!(
        !watch.replaced(later + POLL / 2),
        "looked at most once a second"
    );
    assert!(watch.replaced(later + POLL), "the rename is seen");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_missing_binary_cannot_arm_and_a_vanished_one_is_not_a_replacement() {
    let (dir, path) = binary("missing");
    assert!(ExeWatch::arm(dir.join("nope"), Instant::now()).is_none());
    let start = Instant::now();
    let mut watch = ExeWatch::arm(path.clone(), start).expect("armed");
    std::fs::remove_file(&path).expect("remove");
    assert!(
        !watch.replaced(start + POLL),
        "mid-install, nothing there is not news"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_replaced_binary_asks_and_restart_now_quits_into_it() {
    let (dir, path) = binary("ask");
    let mut app = App::headless(Config::default(), Keymap::builtin(), Theme::blue());
    let start = Instant::now();
    app.exe_watch = ExeWatch::arm(path.clone(), start);

    app.service_exe_watch(start + POLL);
    assert!(app.top_dialog().is_none(), "not until the binary changes");

    install_over(&path);
    app.service_exe_watch(start + POLL * 2);
    assert_eq!(
        app.top_dialog().map(crate::dialog::Dialog::id),
        Some(DialogId::Restart),
        "the question is up"
    );
    assert!(app.exe_watch.is_none(), "and the watch is done");
    assert!(app.restart_target().is_none(), "nothing restarts unasked");

    app.answer_restart(true);
    assert!(app.should_quit, "a restart is a quit first");
    assert_eq!(app.restart_target(), Some(path.as_path()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn later_keeps_this_version_and_quitting_then_does_not_restart() {
    let (dir, path) = binary("later");
    let mut app = App::headless(Config::default(), Keymap::builtin(), Theme::blue());
    let start = Instant::now();
    app.exe_watch = ExeWatch::arm(path.clone(), start);
    install_over(&path);
    app.service_exe_watch(start + POLL);
    app.answer_restart(false);
    assert!(!app.should_quit);
    assert!(
        app.message
            .as_deref()
            .is_some_and(|m| m.contains("next time"))
    );
    app.should_quit = true;
    assert!(app.restart_target().is_none(), "a plain quit stays a quit");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn accepting_the_update_arms_the_watch_on_the_running_binary() {
    let mut app = App::headless(Config::default(), Keymap::builtin(), Theme::blue());
    app.watch_for_new_binary(Instant::now());
    assert!(
        app.exe_watch.is_some(),
        "the test binary itself is readable, so the watch arms"
    );
}
