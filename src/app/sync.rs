//! `Ctrl+Alt+S`: the scan behind the sync view, and running what it chose.
//!
//! The keystroke turns both panels into the sync view and asks for a scan;
//! the event loop starts it on the blocking pool, where both trees are walked
//! through the [`crate::vfs::Vfs`] trait and paired; while it runs the loop
//! passes the counts to the view, and then the rows. The scan lives exactly as
//! long as the view: leaving it cancels the walk.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::app::App;
use crate::app::sync_view::SyncView;
use crate::input::DialogId;
use crate::ops::sync::{self, Direction, Row};
use crate::ops::{JobKind, JobSpec};
use crate::vfs::{Vfs, VfsPath};

/// How often the loop wakes while a scan runs, to show its counts.
const TICK: Duration = Duration::from_millis(100);

/// What the scan sends back.
#[derive(Debug)]
pub enum ScanEvent {
    /// Files found so far on each side.
    Progress {
        /// On the left.
        left: u64,
        /// On the right.
        right: u64,
    },
    /// The rows, and the folders that could not be listed.
    Done {
        /// One per file, in path order.
        rows: Vec<Row>,
        /// Unlistable folders, `left: ` or `right: ` and their relative path.
        unreadable: Vec<String>,
    },
    /// Why it could not finish.
    Failed(String),
    /// The byte check: pairs read, of how many.
    Checking {
        /// Read so far.
        done: usize,
        /// To read.
        total: usize,
    },
}

/// One scan to run.
#[derive(Debug, Clone)]
struct Request {
    left: VfsPath,
    right: VfsPath,
    direction: Direction,
    mirror: bool,
    contents: bool,
}

/// A scan in flight.
#[derive(Debug)]
struct Running {
    rx: mpsc::Receiver<ScanEvent>,
    cancel: Arc<AtomicBool>,
}

impl Drop for Running {
    fn drop(&mut self) {
        // The walk asks between directories, so it stops within one.
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The sync state on the application.
#[derive(Debug, Default)]
pub struct Syncing {
    /// The view in both panels, while it is up.
    pub view: Option<SyncView>,
    /// The jobs a run queued, while they are going.
    awaiting: Vec<crate::ops::JobId>,
    /// The files and bytes the run planned, shown until its jobs have counted
    /// their own.
    planned: (u64, u64),
    /// The compare running now is the one after a run: when it is done, offer
    /// to remember the pair.
    after_run: bool,
    pending: Option<Request>,
    running: Option<Running>,
}

/// Walk both sides and pair them. Runs on the blocking pool.
fn scan(
    vfs: &dyn Vfs,
    request: &Request,
    slack: Duration,
    cancel: &AtomicBool,
    tx: &mpsc::Sender<ScanEvent>,
) {
    let cancelled = || cancel.load(Ordering::Relaxed);
    let mut last = Instant::now();
    let mut counts = (0_u64, 0_u64);
    // Throttled, so a big tree does not flood the channel - but the last count
    // of each side is always sent, below, or a quick listing would show 0.
    let report = |counts: (u64, u64), last: &mut Instant| {
        if last.elapsed() >= TICK {
            *last = Instant::now();
            let _ = tx.send(ScanEvent::Progress {
                left: counts.0,
                right: counts.1,
            });
        }
    };
    let left = sync::list_tree(vfs, &request.left, &cancelled, &mut |n| {
        counts.0 = n;
        report(counts, &mut last);
    });
    let _ = tx.send(ScanEvent::Progress {
        left: counts.0,
        right: counts.1,
    });
    let left = match left {
        Ok(listing) => listing,
        Err(err) => {
            let _ = tx.send(ScanEvent::Failed(format!("{}: {err}", request.left)));
            return;
        }
    };
    let right = sync::list_tree(vfs, &request.right, &cancelled, &mut |n| {
        counts.1 = n;
        report(counts, &mut last);
    });
    let _ = tx.send(ScanEvent::Progress {
        left: counts.0,
        right: counts.1,
    });
    let right = match right {
        Ok(listing) => listing,
        Err(err) => {
            let _ = tx.send(ScanEvent::Failed(format!("{}: {err}", request.right)));
            return;
        }
    };
    let mut rows = sync::diff(&left, &right, slack, request.direction, request.mirror);
    // A file inside a folder the other side could not list is not "only on
    // this side": it is unknown there, and copying or deleting it on that
    // guess is exactly the mistake to avoid. Those rows are dropped.
    let blind = |rel: &str, unreadable: &[String]| {
        unreadable.iter().any(|dir| {
            rel.strip_prefix(dir.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
        })
    };
    rows.retain(|row| !blind(&row.rel, &left.unreadable) && !blind(&row.rel, &right.unreadable));
    if request.contents {
        let checked = sync::check_contents(
            vfs,
            &request.left,
            &request.right,
            &mut rows,
            request.direction,
            request.mirror,
            &cancelled,
            &mut |done, total| {
                let _ = tx.send(ScanEvent::Checking { done, total });
            },
        );
        if checked.is_err() {
            return;
        }
    }
    let unreadable = left
        .unreadable
        .iter()
        .map(|d| format!("left: {d}"))
        .chain(right.unreadable.iter().map(|d| format!("right: {d}")))
        .collect();
    let _ = tx.send(ScanEvent::Done { rows, unreadable });
}

impl App {
    /// `Ctrl+Alt+S`: compare the two panels' directories, in the panels.
    ///
    /// The active panel is the source: one way copies from it to the other.
    /// No I/O here; the scan is queued for the event loop.
    pub fn request_sync(&mut self) {
        let left = self.left.active_tab();
        let right = self.right.active_tab();
        if left.virtual_view.is_some() || right.virtual_view.is_some() {
            self.message =
                Some("sync compares two directories; a search result is not one".to_string());
            return;
        }
        let (left, right) = (left.path.clone(), right.path.clone());
        if left == right {
            self.message = Some("both panels show the same directory".to_string());
            return;
        }
        if left.starts_with(&right) || right.starts_with(&left) {
            self.message = Some(
                "one directory is inside the other; sync would copy it into itself".to_string(),
            );
            return;
        }
        // The panels draw the view; a quick view in one of them would be
        // drawn instead of half of it.
        self.quick = None;
        let direction = self.one_way();
        let mut view = SyncView::new(left.clone(), right.clone(), direction);
        view.set_titles(
            self.location_of(&left).unwrap_or_else(|| left.to_string()),
            self.location_of(&right)
                .unwrap_or_else(|| right.to_string()),
        );
        self.sync.view = Some(view);
        self.sync.running = None;
        self.sync.pending = Some(Request {
            left,
            right,
            direction,
            mirror: false,
            contents: false,
        });
    }

    /// The sync view, while it is up.
    pub fn sync_view(&self) -> Option<&SyncView> {
        self.sync.view.as_ref()
    }

    /// The sync view, mutably.
    pub fn sync_view_mut(&mut self) -> Option<&mut SyncView> {
        self.sync.view.as_mut()
    }

    /// One way, from the active panel to the other.
    pub const fn one_way(&self) -> Direction {
        match self.active_side {
            crate::panel::Side::Left => Direction::ToRight,
            crate::panel::Side::Right => Direction::ToLeft,
        }
    }

    /// Leave the sync view: the panels show their directories again and the
    /// scan, if it is still going, stops.
    pub fn leave_sync(&mut self) {
        self.sync = Syncing::default();
    }

    /// Start a queued scan, pass a running one's news to the view, and stop
    /// it when the view has gone. Called by the event loop every turn.
    pub fn service_sync(&mut self) {
        let slack = self.config.ops.compare_mtime_slack.duration();
        if self.sync.view.is_none() {
            self.sync = Syncing::default();
            return;
        }
        // A run in progress: its jobs' progress, in words, every turn - the
        // view says how far the copy has got, whether its dialog is up or
        // was sent to the background.
        if !self.sync.awaiting.is_empty() {
            let progress = self.run_progress();
            if let Some(view) = self.sync.view.as_mut() {
                view.set_running(progress);
            }
        }
        // A run whose jobs have all finished: compare again, so the panels
        // show what is left to do rather than what was.
        if !self.sync.awaiting.is_empty()
            && self
                .sync
                .awaiting
                .iter()
                .all(|id| self.job(*id).is_none_or(crate::ops::JobStatus::is_finished))
        {
            self.sync.awaiting.clear();
            self.sync.after_run = true;
            if let Some(view) = self.sync.view.as_mut() {
                view.rescan();
            }
        }
        let Some(view) = self.sync.view.as_mut() else {
            return;
        };
        if view.take_rescan() {
            let (left, right) = view.roots();
            self.sync.pending = Some(Request {
                left: left.clone(),
                right: right.clone(),
                direction: view.direction(),
                mirror: view.mirror(),
                contents: view.contents(),
            });
        }
        // Taken only once there is somewhere to run it, so a scan asked for
        // before the runtime is reachable is still owed rather than lost.
        if let Ok(handle) = tokio::runtime::Handle::try_current()
            && let Some(request) = self.sync.pending.take()
        {
            let (tx, rx) = mpsc::channel();
            let cancel = Arc::new(AtomicBool::new(false));
            let flag = Arc::clone(&cancel);
            let vfs = Arc::clone(&self.vfs);
            handle.spawn_blocking(move || scan(vfs.as_ref(), &request, slack, &flag, &tx));
            self.sync.running = Some(Running { rx, cancel });
        }
        let events: Vec<ScanEvent> = self
            .sync
            .running
            .as_ref()
            .map(|running| running.rx.try_iter().collect())
            .unwrap_or_default();
        let finished = events
            .iter()
            .any(|e| matches!(e, ScanEvent::Done { .. } | ScanEvent::Failed(_)));
        if let Some(view) = self.sync.view.as_mut() {
            for event in events {
                apply(view, event);
            }
        }
        if finished {
            self.sync.running = None;
            if std::mem::take(&mut self.sync.after_run) {
                self.offer_to_remember();
            }
        }
    }

    /// When [`App::service_sync`] next wants to run: soon, while a scan is
    /// going, so its counts move.
    pub fn sync_deadline(&self) -> Option<Instant> {
        (self.sync.running.is_some()
            || self.sync.pending.is_some()
            || !self.sync.awaiting.is_empty())
        .then(|| Instant::now() + TICK)
    }

    /// `F5`: ask, with the counts, before running what the panels show.
    pub fn ask_sync(&mut self) {
        let Some(view) = self.sync.view.as_mut() else {
            return;
        };
        if *view.state() != crate::app::sync_view::ScanState::Done {
            self.message = Some("still comparing".to_string());
            return;
        }
        let (to_right, to_left, deletes, _) = view.counts();
        // Refused up front, as `F5` into a read-only backend is: a `.rar`, a
        // disk image or a read-only mount would otherwise fail file by file.
        // What changes on each side: copies onto it, and trashing from it.
        let answer = view.answer();
        let changes_left = to_left > 0
            || answer
                .deletes
                .iter()
                .any(|path| path.starts_with(view.roots().0));
        let changes_right = to_right > 0
            || answer
                .deletes
                .iter()
                .any(|path| path.starts_with(view.roots().1));
        for (changes, side, name) in [
            (changes_left, crate::panel::Side::Left, "left"),
            (changes_right, crate::panel::Side::Right, "right"),
        ] {
            if changes && !self.panel(side).active_tab().caps.writable {
                self.message = Some(format!(
                    "the {name} side is read-only; unmark what would change it"
                ));
                return;
            }
        }
        let Some(view) = self.sync.view.as_ref() else {
            return;
        };
        // Counted, never read off the summary's wording: the summary is for
        // people, and its words change.
        if to_right + to_left + deletes == 0 {
            self.message =
                Some("nothing is marked to synchronize: Space or Ins marks a row".to_string());
            return;
        }
        let summary = view.summary();
        let mut words: Vec<char> = summary.chars().collect();
        if let Some(first) = words.first_mut() {
            *first = first.to_ascii_uppercase();
        }
        let line = format!("{}?", words.into_iter().collect::<String>());
        let dialog = crate::dialog::ConfirmDialog::new(DialogId::Sync, "Synchronize", vec![line])
            .with_buttons("Synchronize", "Cancel");
        // Copying only adds and refreshes, so it is the default answer; a run
        // that trashes files keeps the framework's safe default of Cancel.
        if deletes == 0 {
            self.push_dialog(Box::new(dialog.defaulting_to_yes()));
        } else {
            self.push_dialog(Box::new(dialog));
        }
    }

    /// The question answered yes: queue the copies as one job and the
    /// deletes as another, to the trash. The view stays, says it is
    /// synchronizing, and compares the trees again when the jobs are done.
    pub fn run_sync(&mut self) {
        let Some(view) = self.sync.view.as_ref() else {
            return;
        };
        let answer = view.answer();
        // A run with nothing in it would wait for jobs that were never queued.
        if answer.copies.is_empty() && answer.deletes.is_empty() {
            self.message = Some("nothing is marked to synchronize".to_string());
            return;
        }
        let (_, _, _, planned_bytes) = view.counts();
        self.sync.planned = (
            u64::try_from(answer.copies.len() + answer.deletes.len()).unwrap_or(u64::MAX),
            planned_bytes,
        );
        let copies = answer.copies.len();
        let deletes = answer.deletes.len();
        let mut awaiting = Vec::new();
        if copies > 0 {
            let spec = JobSpec::sync(answer.copies).with_options(crate::ops::JobOptions {
                conflict: Some(crate::ops::ConflictChoice::Overwrite),
                ..crate::ops::JobOptions::from_config(&self.config.ops)
            });
            awaiting.push(self.request_job(spec));
        }
        if deletes > 0 {
            awaiting.push(self.request_job(JobSpec::new(
                JobKind::Delete { trash: true },
                answer.deletes,
                None,
            )));
        }
        self.sync.awaiting = awaiting;
        let progress = self.run_progress();
        if let Some(view) = self.sync.view.as_mut() {
            view.set_running(progress);
        }
        self.message = Some(match (copies, deletes) {
            (0, 0) => "nothing to synchronize".to_string(),
            (c, 0) => format!("synchronizing {c} file{}", if c == 1 { "" } else { "s" }),
            (0, d) => format!(
                "moving {d} file{} to the trash",
                if d == 1 { "" } else { "s" }
            ),
            (c, d) => format!(
                "synchronizing {c} file{}, moving {d} to the trash",
                if c == 1 { "" } else { "s" }
            ),
        });
    }
}

impl App {
    /// How far a run has got: `copying 3 of 12 files, 41 M of 122 M:
    /// report.pdf`, from the jobs it queued.
    fn run_progress(&self) -> String {
        let mut files = (0_u64, 0_u64);
        let mut bytes = (0_u64, 0_u64);
        let mut current = String::new();
        for id in &self.sync.awaiting {
            let Some(job) = self.job(*id) else {
                continue;
            };
            files.0 = files.0.saturating_add(job.files_done);
            files.1 = files.1.saturating_add(job.files_total);
            bytes.0 = bytes.0.saturating_add(job.bytes_done);
            bytes.1 = bytes.1.saturating_add(job.bytes_total);
            if job.is_running() && !job.file.is_empty() {
                current = job.file.rsplit('/').next().unwrap_or(&job.file).to_string();
            }
        }
        // A job that has not started has counted nothing yet; the plan is the
        // honest total until it has.
        files.1 = files.1.max(self.sync.planned.0);
        bytes.1 = bytes.1.max(self.sync.planned.1);
        let size = crate::panel::format::human_size;
        let mut line = format!("synchronizing {} of {} files", files.0, files.1);
        if bytes.1 > 0 {
            line.push_str(&format!(", {} of {}", size(bytes.0), size(bytes.1)));
        }
        if !current.is_empty() {
            line.push_str(&format!(": {current}"));
        }
        line
    }
}

fn apply(view: &mut SyncView, event: ScanEvent) {
    match event {
        ScanEvent::Progress { left, right } => view.set_progress(left, right),
        ScanEvent::Done { rows, unreadable } => view.set_rows(rows, unreadable),
        ScanEvent::Failed(why) => view.set_failed(why),
        ScanEvent::Checking { done, total } => view.set_checking(done, total),
    }
}

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;
