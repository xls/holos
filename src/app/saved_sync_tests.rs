use super::*;
use crate::config::{Config, Keymap, Theme};

fn app_at(left: &str, right: &str) -> App {
    let mut app = App::headless(Config::default(), Keymap::builtin(), Theme::blue());
    app.left.active_tab_mut().path = VfsPath::local(left);
    app.right.active_tab_mut().path = VfsPath::local(right);
    app
}

fn pair(left: &str, right: &str) -> SavedSync {
    SavedSync::new(left.to_string(), right.to_string())
}

#[test]
fn the_file_round_trips_and_a_broken_one_is_a_warning() {
    let list = vec![
        pair("/home/t/projects", "sftp://t@nas.local:22/backup"),
        pair("/home/t/photos", "/mnt/usb/photos"),
    ];
    let text = render(&list).unwrap();
    assert!(text.starts_with("# Saved syncs"));
    assert_eq!(parse(&text), (list, Vec::new()));
    let (none, warnings) = parse("this is = = not toml");
    assert!(none.is_empty());
    assert_eq!(warnings.len(), 1);
}

#[test]
fn a_remote_address_reads_back_as_its_host_and_directory_and_never_a_password() {
    let Ok(Location::Remote(target)) = parse_location("sftp://thorin@nas.local:2222/srv/backup")
    else {
        panic!("an address");
    };
    assert_eq!(target.host, "nas.local");
    assert_eq!(target.port, 2222);
    assert_eq!(target.user, "thorin");
    assert_eq!(target.dir.as_deref(), Some("/srv/backup"));
    assert_eq!(
        parse_location("/home/t"),
        Ok(Location::Local(std::path::PathBuf::from("/home/t")))
    );
}

#[test]
fn ctrl_alt_s_with_nothing_saved_compares_the_panels_straight_away() {
    let mut app = app_at("/a", "/b");
    app.open_sync();
    assert!(app.sync_view().is_some());
    assert!(app.dialogs().is_empty());
}

#[test]
fn with_a_saved_pair_it_offers_the_panels_and_the_pair() {
    let mut app = app_at("/a", "/b");
    app.saved_syncs.add(pair("/x", "/y"));
    app.open_sync();
    assert!(app.sync_view().is_none());
    assert_eq!(app.top_dialog().map(|d| d.id()), Some(DialogId::SyncPicker));
}

#[test]
fn after_a_run_it_asks_once_and_remembers_on_yes() {
    let mut app = app_at("/a", "/b");
    app.offer_to_remember();
    assert_eq!(
        app.top_dialog().map(|d| d.id()),
        Some(DialogId::SyncRemember)
    );
    app.pop_dialog();
    app.answer_remember(true);
    assert_eq!(app.saved_syncs().list(), [pair("/a", "/b")]);
    assert_eq!(app.take_saved_syncs_write(), Some(vec![pair("/a", "/b")]));
    assert_eq!(app.take_saved_syncs_write(), None, "written once");
    // Saved now, so the next run does not ask again.
    app.offer_to_remember();
    assert!(app.dialogs().is_empty());
}

#[test]
fn not_now_saves_nothing() {
    let mut app = app_at("/a", "/b");
    app.offer_to_remember();
    app.pop_dialog();
    app.answer_remember(false);
    assert!(app.saved_syncs().list().is_empty());
}

#[test]
fn opening_a_saved_pair_moves_both_panels_and_compares_when_they_arrive() {
    let mut app = app_at("/elsewhere", "/else");
    app.saved_syncs.add(pair("/x", "/y"));
    app.answer_sync_pick(crate::ui::dialog::syncpick::Pick::Open(0));
    assert_eq!(app.left.active_tab().path, VfsPath::local("/x"));
    assert_eq!(app.right.active_tab().path, VfsPath::local("/y"));
    // Not until both listings have arrived.
    app.service_saved_open();
    assert!(app.sync_view().is_none());
    app.left.active_tab_mut().loading = false;
    app.right.active_tab_mut().loading = false;
    app.service_saved_open();
    assert!(app.sync_view().is_some());
    assert_eq!(
        app.sync_view().map(|v| v.roots().1.clone()),
        Some(VfsPath::local("/y"))
    );
}

#[test]
fn a_remote_side_with_no_connection_is_connected_on_its_own_panel() {
    let mut app = app_at("/elsewhere", "/else");
    app.saved_syncs
        .add(pair("/x", "sftp://thorin@nas.local:22/backup"));
    app.answer_sync_pick(crate::ui::dialog::syncpick::Pick::Open(0));
    let request = app.take_pending_connect().expect("a connection is started");
    assert_eq!(request.side, Side::Right);
    assert_eq!(request.answer.target.host, "nas.local");
    assert_eq!(request.answer.target.dir.as_deref(), Some("/backup"));
    assert!(
        request.answer.password.is_none(),
        "an address carries no password"
    );
}

#[test]
fn a_nested_place_cannot_be_saved() {
    let app = app_at("/a", "/b");
    let inside = VfsPath::local("/a/x.zip").with_segment(BackendKind::Archive, "/dir");
    assert_eq!(app.location_of(&inside), None);
}

#[test]
fn a_pair_is_named_by_its_folders_until_f2_names_it() {
    let mut app = app_at("/a", "/b");
    app.saved_syncs
        .add(pair("/home/t/projects", "sftp://t@nas.local:22/srv/backup"));
    assert_eq!(
        app.saved_syncs().list()[0].shown_name(true),
        "projects <-> backup"
    );

    app.open_sync();
    app.answer_sync_pick(crate::ui::dialog::syncpick::Pick::Rename(0));
    assert_eq!(app.top_dialog().map(|d| d.id()), Some(DialogId::SyncRename));
    app.pop_dialog();
    app.answer_sync_rename("Work backup");
    assert_eq!(app.saved_syncs().list()[0].shown_name(true), "Work backup");
    assert!(
        app.take_saved_syncs_write().is_some(),
        "and the file is rewritten"
    );
    // The name round-trips through the file, and an empty one is the default.
    let text = render(app.saved_syncs().list()).unwrap();
    assert!(text.contains("name = \"Work backup\""), "{text}");
    app.saved_syncs.rename(0, "  ");
    assert_eq!(app.saved_syncs().list()[0].name, None);
}
