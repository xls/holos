use super::*;
use crate::ops::copy::tests::{TempTree, drive};
use crate::vfs::LocalFs;
use std::time::UNIX_EPOCH;

const SLACK: Duration = Duration::from_secs(2);

fn facts(size: u64, secs: u64) -> Facts {
    Facts {
        size,
        mtime: Some(UNIX_EPOCH + Duration::from_secs(secs)),
    }
}

fn listing(files: &[(&str, u64, u64)], dirs: &[&str]) -> Listing {
    Listing {
        files: files
            .iter()
            .map(|(rel, size, secs)| ((*rel).to_string(), facts(*size, *secs)))
            .collect(),
        dirs: dirs.iter().map(|d| (*d).to_string()).collect(),
        unreadable: Vec::new(),
    }
}

#[test]
fn the_newer_date_wins_and_the_slack_forgives_a_rounded_stamp() {
    assert_eq!(
        compare(facts(1, 100), facts(1, 50), SLACK),
        Status::LeftNewer
    );
    assert_eq!(
        compare(facts(1, 50), facts(9, 100), SLACK),
        Status::RightNewer
    );
    // FAT's two-second stamp: the same file.
    assert_eq!(compare(facts(7, 100), facts(7, 101), SLACK), Status::Same);
    // Same date, different size: nothing says which to keep.
    assert_eq!(
        compare(facts(7, 100), facts(8, 100), SLACK),
        Status::Differs
    );
    // A side without a date cannot be newer.
    let undated = Facts {
        size: 7,
        mtime: None,
    };
    assert_eq!(compare(undated, facts(7, 100), SLACK), Status::Same);
    assert_eq!(compare(undated, facts(8, 100), SLACK), Status::Differs);
}

#[test]
fn diff_pairs_by_path_and_says_what_each_side_is_missing() {
    let left = listing(
        &[("a.txt", 1, 100), ("docs/new.md", 2, 100), ("same", 3, 100)],
        &["docs"],
    );
    let right = listing(
        &[("a.txt", 1, 300), ("old/x", 4, 100), ("same", 3, 100)],
        &["old"],
    );
    let rows = diff(&left, &right, SLACK, Direction::ToRight, false);
    let got: Vec<(&str, Status, Plan)> = rows
        .iter()
        .map(|r| (r.rel.as_str(), r.status, r.plan))
        .collect();
    assert_eq!(
        got,
        [
            ("a.txt", Status::RightNewer, Plan::Skip),
            ("docs/new.md", Status::OnlyLeft, Plan::ToRight),
            ("old/x", Status::OnlyRight, Plan::Skip),
            ("same", Status::Same, Plan::Skip),
        ]
    );
}

#[test]
fn a_file_where_the_other_side_has_a_directory_is_a_clash_and_never_copied() {
    let left = listing(&[("thing", 1, 100)], &[]);
    let right = listing(&[("thing/inside", 1, 100)], &["thing"]);
    let rows = diff(&left, &right, SLACK, Direction::Both, false);
    assert_eq!(rows.len(), 2);
    let thing = rows.iter().find(|r| r.rel == "thing").unwrap();
    assert_eq!(thing.status, Status::Clash);
    assert_eq!(thing.plan, Plan::Skip);
    assert_eq!(thing.choices(), [Plan::Skip], "not even by hand");
}

#[test]
fn one_way_copies_what_is_new_and_mirror_makes_the_target_identical() {
    use Status::*;
    let one = |s| default_plan(s, Direction::ToRight, false);
    assert_eq!(one(OnlyLeft), Plan::ToRight);
    assert_eq!(one(LeftNewer), Plan::ToRight);
    assert_eq!(
        one(RightNewer),
        Plan::Skip,
        "a newer backup is not overwritten"
    );
    assert_eq!(one(OnlyRight), Plan::Skip, "nothing is deleted");
    assert_eq!(one(Differs), Plan::Skip);
    assert_eq!(one(Same), Plan::Skip);

    let mirror = |s| default_plan(s, Direction::ToRight, true);
    assert_eq!(mirror(RightNewer), Plan::ToRight);
    assert_eq!(mirror(Differs), Plan::ToRight);
    assert_eq!(mirror(OnlyRight), Plan::DeleteRight);
    assert_eq!(mirror(Same), Plan::Skip);

    // The same rules the other way round.
    assert_eq!(
        default_plan(OnlyRight, Direction::ToLeft, false),
        Plan::ToLeft
    );
    assert_eq!(
        default_plan(OnlyLeft, Direction::ToLeft, true),
        Plan::DeleteLeft
    );
}

#[test]
fn both_ways_takes_the_newer_side_and_never_deletes() {
    use Status::*;
    for mirror in [false, true] {
        let both = |s| default_plan(s, Direction::Both, mirror);
        assert_eq!(both(OnlyLeft), Plan::ToRight);
        assert_eq!(both(OnlyRight), Plan::ToLeft);
        assert_eq!(both(LeftNewer), Plan::ToRight);
        assert_eq!(both(RightNewer), Plan::ToLeft);
        assert_eq!(both(Differs), Plan::Skip, "left for the user to choose");
    }
}

#[test]
fn space_steps_through_only_the_plans_a_row_can_take() {
    let mut row = Row {
        rel: "only-left".to_string(),
        left: Some(facts(1, 1)),
        right: None,
        status: Status::OnlyLeft,
        plan: Plan::ToRight,
    };
    assert_eq!(row.choices(), [Plan::Skip, Plan::ToRight, Plan::DeleteLeft]);
    row.cycle();
    assert_eq!(row.plan, Plan::DeleteLeft);
    row.cycle();
    assert_eq!(row.plan, Plan::Skip);
    row.cycle();
    assert_eq!(row.plan, Plan::ToRight);
}

#[test]
fn a_tree_is_listed_by_relative_path_through_the_vfs() {
    let tree = TempTree::new("sync-list");
    tree.file("a.txt", b"one");
    tree.file("sub/deeper/b.bin", b"three");
    tree.dir("empty");
    // A local listing streams from the blocking pool, as it does in the app.
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _inside = rt.enter();
    let fs = LocalFs::new();
    let mut seen = 0;
    let got = list_tree(&fs, &VfsPath::local(tree.path()), &|| false, &mut |n| {
        seen = n
    })
    .unwrap();
    let files: Vec<&str> = got.files.keys().map(String::as_str).collect();
    assert_eq!(files, ["a.txt", "sub/deeper/b.bin"]);
    assert_eq!(got.files["sub/deeper/b.bin"].size, 5);
    assert!(got.dirs.contains("sub/deeper") && got.dirs.contains("empty"));
    assert_eq!(seen, 2, "progress counts files");
}

#[test]
fn a_cancelled_listing_says_so() {
    let tree = TempTree::new("sync-cancel");
    tree.file("a", b"1");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _inside = rt.enter();
    let fs = LocalFs::new();
    let got = list_tree(&fs, &VfsPath::local(tree.path()), &|| true, &mut |_| {});
    assert!(matches!(got, Err(Error::Cancelled)));
}

#[test]
fn the_contents_check_finds_same_size_same_date_files_that_differ() {
    let left = TempTree::new("sync-bytes-l");
    let right = TempTree::new("sync-bytes-r");
    left.file("x", b"aaaa");
    right.file("x", b"aaab");
    left.file("y", b"same");
    right.file("y", b"same");
    let mut rows: Vec<Row> = ["x", "y"]
        .iter()
        .map(|rel| Row {
            rel: (*rel).to_string(),
            left: Some(facts(4, 1)),
            right: Some(facts(4, 1)),
            status: Status::Same,
            plan: Plan::Skip,
        })
        .collect();
    let fs = LocalFs::new();
    let unreadable = check_contents(
        &fs,
        &VfsPath::local(left.path()),
        &VfsPath::local(right.path()),
        &mut rows,
        Direction::ToRight,
        true,
        &|| false,
    )
    .unwrap();
    assert_eq!(unreadable, 0);
    assert_eq!(rows[0].status, Status::Differs);
    assert_eq!(rows[0].plan, Plan::ToRight, "mirror overwrites it");
    assert_eq!(rows[1].status, Status::Same);
}

#[test]
fn the_job_copies_onto_exact_paths_making_directories_and_keeping_the_date() {
    let left = TempTree::new("sync-run-l");
    let right = TempTree::new("sync-run-r");
    let src_new = left.file("deep/er/new.txt", b"brand new");
    let src_old = left.file("over.txt", b"fresh contents");
    right.file("over.txt", b"stale");
    let stamp = UNIX_EPOCH + Duration::from_secs(1_600_000_000);
    std::fs::File::options()
        .write(true)
        .open(&src_new)
        .unwrap()
        .set_modified(stamp)
        .unwrap();

    let summary = drive(JobSpec::sync(vec![
        (
            VfsPath::local(&src_new),
            VfsPath::local(right.path().join("deep/er/new.txt")),
        ),
        (
            VfsPath::local(&src_old),
            VfsPath::local(right.path().join("over.txt")),
        ),
    ]));
    assert!(summary.failures.is_empty(), "{:?}", summary.failures);
    assert_eq!(summary.files_done, 2);
    let copied = right.path().join("deep/er/new.txt");
    assert_eq!(std::fs::read(&copied).unwrap(), b"brand new");
    assert_eq!(
        std::fs::read(right.path().join("over.txt")).unwrap(),
        b"fresh contents"
    );
    // The date travels, so the next comparison calls the pair the same.
    assert_eq!(
        std::fs::metadata(&copied).unwrap().modified().unwrap(),
        stamp
    );
}

#[test]
fn a_source_that_has_gone_is_one_failure_and_the_rest_still_copy() {
    let left = TempTree::new("sync-gone-l");
    let right = TempTree::new("sync-gone-r");
    let there = left.file("there", b"1");
    let summary = drive(JobSpec::sync(vec![
        (
            VfsPath::local(left.path().join("gone")),
            VfsPath::local(right.path().join("gone")),
        ),
        (
            VfsPath::local(&there),
            VfsPath::local(right.path().join("there")),
        ),
    ]));
    assert_eq!(summary.failures.len(), 1);
    assert_eq!(summary.files_done, 1);
    assert!(right.path().join("there").exists());
}
