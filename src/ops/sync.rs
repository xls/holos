//! Synchronising two directory trees (`Ctrl+Alt+S`).
//!
//! Three steps, each its own function so each is tested on its own:
//!
//! 1. [`list_tree`] walks one side through the [`Vfs`] trait, so a local
//!    folder, an SFTP host, an S3 bucket and an archive are all the same to
//!    it, and returns every file under the root by its path relative to it.
//! 2. [`diff`] pairs the two listings by relative path and says, per file,
//!    which side is newer, which side it is missing from, or that the two
//!    cannot be told apart by size and date. It is pure: two listings in,
//!    rows out, no filesystem.
//! 3. [`run`] is the [`crate::ops::JobKind::Sync`] worker: it copies each
//!    chosen file onto its exact counterpart path, making the directories on
//!    the way, as a job with progress and cancel like any copy.
//!
//! What is copied and in which direction is decided by [`default_plan`] and
//! then by the user, row by row, in the sync dialog. Deleting is never a
//! default except under mirror, and goes through the ordinary delete job, to
//! the trash.
//!
//! Directories are not rows. A directory that exists on one side only shows
//! up as the files inside it, and is made by the copy that needs it; an empty
//! directory has nothing to synchronise and is left alone.

use std::collections::{BTreeMap, HashSet};
use std::time::{Duration, SystemTime};

use crate::error::Error;
use crate::vfs::{EntryKind, Vfs, VfsPath};

use super::{JobContext, JobKind, JobSpec, is_fatal};

/// What one side knows about a file: enough to compare it, and to size the
/// copy's progress bar before a byte is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Facts {
    /// Its size in bytes.
    pub size: u64,
    /// When it was last modified, when the backend says.
    pub mtime: Option<SystemTime>,
}

/// Every file under one root, by relative path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listing {
    /// Files, keyed by their path below the root with `/` between levels.
    /// A `BTreeMap` so the rows come out in path order without a sort.
    pub files: BTreeMap<String, Facts>,
    /// Directories, by the same key. Only needed to spot a file on one side
    /// standing where a directory is on the other.
    pub dirs: HashSet<String>,
    /// Directories that could not be listed, by the same key. Their contents
    /// are missing from `files`, so a file in one is not "only on the other
    /// side" - the dialog says which ones they were.
    pub unreadable: Vec<String>,
}

/// How one path compares across the two sides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    /// Same size, and dates within the slack (or a side without a date).
    Same,
    /// The left copy is newer.
    LeftNewer,
    /// The right copy is newer.
    RightNewer,
    /// Only on the left.
    OnlyLeft,
    /// Only on the right.
    OnlyRight,
    /// Different sizes with dates that do not say which is newer, or the
    /// contents check found different bytes. Nothing is decided for it.
    Differs,
    /// A file on one side where the other has a directory. Copying either
    /// way would have to delete a tree first, which sync does not do.
    Clash,
}

impl Status {
    /// The one-character sign the dialog draws between the two sides.
    #[must_use]
    pub const fn sign(self, ascii: bool) -> &'static str {
        match (self, ascii) {
            (Self::Same, _) => "=",
            (Self::LeftNewer | Self::OnlyLeft, false) => "\u{2192}",
            (Self::LeftNewer | Self::OnlyLeft, true) => ">",
            (Self::RightNewer | Self::OnlyRight, false) => "\u{2190}",
            (Self::RightNewer | Self::OnlyRight, true) => "<",
            (Self::Differs, false) => "\u{2260}",
            (Self::Differs, true) => "!",
            (Self::Clash, _) => "x",
        }
    }
}

/// What will be done to one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Plan {
    /// Nothing.
    Skip,
    /// Copy the left file over the right one, or to where it is missing.
    ToRight,
    /// Copy the right file over the left one, or to where it is missing.
    ToLeft,
    /// Delete the left file (to the trash).
    DeleteLeft,
    /// Delete the right file (to the trash).
    DeleteRight,
}

/// Which way a run goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Make the right side up to date from the left.
    ToRight,
    /// Make the left side up to date from the right.
    ToLeft,
    /// Each side takes what is newer on the other.
    Both,
}

/// One path and what is known about it on each side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The path below both roots, `/` between levels.
    pub rel: String,
    /// The left file, if there is one.
    pub left: Option<Facts>,
    /// The right file, if there is one.
    pub right: Option<Facts>,
    /// How the two compare.
    pub status: Status,
    /// What will be done.
    pub plan: Plan,
}

impl Row {
    /// The plans this row can take, in the order `Space` steps through them.
    /// A copy needs a source and a delete needs a target, so a file on one
    /// side only cannot be copied from the other.
    #[must_use]
    pub fn choices(&self) -> Vec<Plan> {
        if self.status == Status::Clash {
            return vec![Plan::Skip];
        }
        let mut out = vec![Plan::Skip];
        if self.left.is_some() {
            out.push(Plan::ToRight);
        }
        if self.right.is_some() {
            out.push(Plan::ToLeft);
        }
        if self.left.is_some() && self.right.is_none() {
            out.push(Plan::DeleteLeft);
        }
        if self.right.is_some() && self.left.is_none() {
            out.push(Plan::DeleteRight);
        }
        out
    }

    /// `Space`: the next plan this row can take.
    pub fn cycle(&mut self) {
        let choices = self.choices();
        let at = choices.iter().position(|p| *p == self.plan).unwrap_or(0);
        self.plan = choices
            .get(at.saturating_add(1))
            .or_else(|| choices.first())
            .copied()
            .unwrap_or(Plan::Skip);
    }
}

/// What a run in `direction` does to a row of `status` before anyone
/// touches it.
///
/// One way, only the target side changes, and only where the source is newer
/// or the target is missing the file. `mirror` makes the target identical to
/// the source instead: a newer or different target is overwritten, and a file
/// the source does not have is deleted. Both ways, each side takes what is
/// newer on the other and nothing is ever deleted; a pair that cannot be told
/// apart by date is left for the user to choose.
#[must_use]
pub const fn default_plan(status: Status, direction: Direction, mirror: bool) -> Plan {
    match direction {
        Direction::ToRight => match status {
            Status::OnlyLeft | Status::LeftNewer => Plan::ToRight,
            Status::RightNewer | Status::Differs if mirror => Plan::ToRight,
            Status::OnlyRight if mirror => Plan::DeleteRight,
            Status::RightNewer
            | Status::Differs
            | Status::OnlyRight
            | Status::Same
            | Status::Clash => Plan::Skip,
        },
        Direction::ToLeft => match status {
            Status::OnlyRight | Status::RightNewer => Plan::ToLeft,
            Status::LeftNewer | Status::Differs if mirror => Plan::ToLeft,
            Status::OnlyLeft if mirror => Plan::DeleteLeft,
            Status::LeftNewer
            | Status::Differs
            | Status::OnlyLeft
            | Status::Same
            | Status::Clash => Plan::Skip,
        },
        Direction::Both => match status {
            Status::OnlyLeft | Status::LeftNewer => Plan::ToRight,
            Status::OnlyRight | Status::RightNewer => Plan::ToLeft,
            Status::Differs | Status::Same | Status::Clash => Plan::Skip,
        },
    }
}

/// How two copies of one file compare, by size and date.
///
/// `slack` is `ops.compare_mtime_slack`, the same tolerance `Shift+F2` uses:
/// FAT keeps two-second times and FTP often whole minutes, so a copy made a
/// moment ago can carry a slightly different stamp. A side without a date
/// cannot be newer, so a pair where either has none is the same when the
/// sizes agree and different when they do not.
#[must_use]
pub fn compare(left: Facts, right: Facts, slack: Duration) -> Status {
    let order = match (left.mtime, right.mtime) {
        (Some(l), Some(r)) => match l.duration_since(r) {
            Ok(ahead) if ahead > slack => Some(Status::LeftNewer),
            Ok(_) => None,
            Err(behind) if behind.duration() > slack => Some(Status::RightNewer),
            Err(_) => None,
        },
        _ => None,
    };
    match order {
        Some(newer) => newer,
        None if left.size == right.size => Status::Same,
        None => Status::Differs,
    }
}

/// Pair two listings by relative path.
///
/// Every row starts with the plan [`default_plan`] gives it for `direction`.
#[must_use]
pub fn diff(
    left: &Listing,
    right: &Listing,
    slack: Duration,
    direction: Direction,
    mirror: bool,
) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut l = left.files.iter().peekable();
    let mut r = right.files.iter().peekable();
    // A merge of two sorted maps: one pass, no lookups.
    loop {
        let (rel, lf, rf) = match (l.peek(), r.peek()) {
            (None, None) => break,
            (Some((lk, _)), Some((rk, _))) if lk == rk => {
                let (rel, lf) = l
                    .next()
                    .map_or((String::new(), None), |(k, v)| (k.clone(), Some(*v)));
                let rf = r.next().map(|(_, v)| *v);
                (rel, lf, rf)
            }
            (Some((lk, _)), Some((rk, _))) if lk < rk => {
                let (rel, lf) = l
                    .next()
                    .map_or((String::new(), None), |(k, v)| (k.clone(), Some(*v)));
                (rel, lf, None)
            }
            (Some(_), None) => {
                let (rel, lf) = l
                    .next()
                    .map_or((String::new(), None), |(k, v)| (k.clone(), Some(*v)));
                (rel, lf, None)
            }
            (_, Some(_)) => {
                let (rel, rf) = r
                    .next()
                    .map_or((String::new(), None), |(k, v)| (k.clone(), Some(*v)));
                (rel, None, rf)
            }
        };
        let status = match (lf, rf) {
            (Some(a), Some(b)) => compare(a, b, slack),
            (Some(_), None) if right.dirs.contains(&rel) => Status::Clash,
            (None, Some(_)) if left.dirs.contains(&rel) => Status::Clash,
            (Some(_), None) => Status::OnlyLeft,
            (None, Some(_)) => Status::OnlyRight,
            (None, None) => continue,
        };
        rows.push(Row {
            plan: default_plan(status, direction, mirror),
            rel,
            left: lf,
            right: rf,
            status,
        });
    }
    rows
}

/// `root` with a relative path below it, one level at a time, so each name
/// goes through [`VfsPath::join`] like a name from a listing.
#[must_use]
pub fn under(root: &VfsPath, rel: &str) -> VfsPath {
    rel.split('/')
        .filter(|level| !level.is_empty())
        .fold(root.clone(), |path, level| path.join(level))
}

/// Every file under `root`.
///
/// `cancelled` is asked between directories; `progress` is told the running
/// count of files found. A directory that cannot be listed is recorded in
/// [`Listing::unreadable`] and the walk goes on; an error that means the
/// connection is gone ([`is_fatal`]) stops it, because every directory left
/// would fail the same way.
///
/// # Errors
///
/// The root itself cannot be read, the walk was cancelled, or the backend
/// went away during it.
pub fn list_tree(
    vfs: &dyn Vfs,
    root: &VfsPath,
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(u64),
) -> crate::Result<Listing> {
    let mut out = Listing::default();
    // An explicit stack: the depth of a tree is not ours to bound.
    let mut stack: Vec<String> = vec![String::new()];
    let mut found = 0_u64;
    while let Some(rel) = stack.pop() {
        if cancelled() {
            return Err(Error::Cancelled);
        }
        let dir = under(root, &rel);
        let mut rx = vfs.read_dir(&dir);
        let mut failed = None;
        while let Some(item) = rx.blocking_recv() {
            match item {
                Ok(entry) if entry.is_parent => {}
                Ok(entry) => {
                    let child = if rel.is_empty() {
                        entry.name.clone()
                    } else {
                        format!("{rel}/{}", entry.name)
                    };
                    if matches!(entry.kind, EntryKind::Dir) {
                        out.dirs.insert(child.clone());
                        stack.push(child);
                    } else {
                        out.files.insert(
                            child,
                            Facts {
                                size: entry.size,
                                mtime: entry.mtime,
                            },
                        );
                        found = found.saturating_add(1);
                    }
                }
                Err(err) => failed = Some(err),
            }
        }
        if let Some(err) = failed {
            if rel.is_empty() || is_fatal(&err) {
                return Err(err);
            }
            out.unreadable.push(rel);
        }
        progress(found);
    }
    Ok(out)
}

/// Read both sides of every row the dates and sizes call the same, and
/// mark the ones whose bytes differ as [`Status::Differs`].
///
/// A pair that cannot be read is left as it was: "could not be read" is not
/// "differs". Returns how many pairs could not be read.
///
/// # Errors
///
/// Cancelled.
pub fn check_contents(
    vfs: &dyn Vfs,
    left_root: &VfsPath,
    right_root: &VfsPath,
    rows: &mut [Row],
    direction: Direction,
    mirror: bool,
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(usize, usize),
) -> crate::Result<usize> {
    let mut unreadable = 0_usize;
    let total = rows.iter().filter(|row| row.status == Status::Same).count();
    progress(0, total);
    for (done, row) in rows
        .iter_mut()
        .filter(|row| row.status == Status::Same)
        .enumerate()
    {
        if cancelled() {
            return Err(Error::Cancelled);
        }
        progress(done, total);
        let a = under(left_root, &row.rel);
        let b = under(right_root, &row.rel);
        let mut tick = |_: u64| !cancelled();
        match super::compare::first_difference(vfs, &a, &b, &mut tick) {
            Ok(Some(_)) => {
                row.status = Status::Differs;
                row.plan = default_plan(Status::Differs, direction, mirror);
            }
            Ok(None) => {}
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(_) => unreadable = unreadable.saturating_add(1),
        }
    }
    Ok(unreadable)
}

impl JobSpec {
    /// A [`JobKind::Sync`]: each source copied onto the target beside it.
    pub fn sync(pairs: Vec<(VfsPath, VfsPath)>) -> Self {
        let (sources, targets): (Vec<VfsPath>, Vec<VfsPath>) = pairs.into_iter().unzip();
        Self {
            kind: JobKind::Sync,
            sources,
            dest: None,
            options: super::JobOptions::default(),
            targets,
        }
    }
}

/// The [`JobKind::Sync`] worker.
///
/// Each source replaces its target, through the same one-file copy every
/// cross-backend copy uses: a local target is written to a partial file,
/// synced and renamed over the old one, so a cancel or a failure never
/// leaves it half-written; mode and date are carried where the target can
/// take them, so the next comparison sees the two as the same. A failure
/// is one line in the summary and the batch goes on, unless the connection
/// went away.
pub fn run(vfs: &dyn Vfs, spec: &JobSpec, ctx: &mut JobContext) {
    debug_assert_eq!(spec.kind, JobKind::Sync);
    let pairs: Vec<(&VfsPath, &VfsPath)> = spec.sources.iter().zip(&spec.targets).collect();
    // One `stat` per source, kept: the total for the bar, and the entry the
    // copy carries the mode and date from.
    let entries: Vec<_> = pairs.iter().map(|(src, _)| vfs.stat(src)).collect();
    let bytes = entries
        .iter()
        .filter_map(|e| e.as_ref().ok())
        .fold(0_u64, |acc, e| acc.saturating_add(e.size));
    ctx.start(u64::try_from(pairs.len()).unwrap_or(u64::MAX), bytes);

    for ((src, dst), entry) in pairs.into_iter().zip(entries) {
        if ctx.cancelled() {
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                let stop = is_fatal(&err);
                ctx.fail(src, err);
                if stop {
                    break;
                }
                continue;
            }
        };
        ctx.set_file(&src.to_string(), entry.size);
        let made = dst.parent().map_or(Ok(()), |dir| {
            super::copy::vfs::ensure_dir_via_vfs(vfs, &dir)
        });
        let copied = made.and_then(|()| {
            super::copy::vfs::copy_one_via_vfs(vfs, src, dst, &entry, &spec.options, ctx)
        });
        match copied {
            Ok(()) => ctx.add_file(),
            Err(Error::Cancelled) => break,
            Err(err) => {
                let stop = is_fatal(&err);
                ctx.fail(src, err);
                if stop {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;
