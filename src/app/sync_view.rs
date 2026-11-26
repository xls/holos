//! The sync view's state: the two compared trees, shown in the two panels.
//!
//! One row per file, the same row in both panels, so a file always faces its
//! counterpart; the two-character border between the panels says what will
//! be done to it. Files that differ start **marked**, like a panel selection:
//! the marked rows are what the run copies, and unmarking one leaves it
//! alone. `Left` and `Right` point a row the other way.
//!
//! Pure state, no I/O: the scan behind it is [`crate::app::sync`]'s, and the
//! drawing is [`crate::ui::syncview`]'s.

use crate::ops::sync::{Direction, Plan, Row, Status, default_plan, under};
use crate::panel::format::human_size;
use crate::vfs::VfsPath;

/// What the run should do: the copies and deletes the marked rows plan.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SyncAnswer {
    /// Each source and the exact path it is copied onto.
    pub copies: Vec<(VfsPath, VfsPath)>,
    /// Files to move to the trash.
    pub deletes: Vec<VfsPath>,
}

/// Where the scan is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanState {
    /// Still walking: files found on each side so far.
    Scanning {
        /// On the left.
        left: u64,
        /// On the right.
        right: u64,
    },
    /// The rows are in.
    Done,
    /// The run `F5` started is copying; the trees are compared again when it
    /// finishes, so what is left to do - ideally nothing - is what is shown.
    /// The text is the run's progress, refreshed from its job every turn.
    Running(String),
    /// The listings are in and the byte check is reading pairs.
    Checking {
        /// Pairs read so far.
        done: usize,
        /// Pairs to read.
        total: usize,
    },
    /// The scan could not finish, and why.
    Failed(String),
}

/// How a run goes, switched with `Alt+M`; the arrows follow it at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The target takes what is newer or missing from the source.
    OneWay,
    /// Each side takes what is newer on the other.
    Both,
    /// The target becomes identical to the source: overwritten where it
    /// differs, and its extra files trashed.
    Mirror,
}

/// The sync view.
#[derive(Debug, Clone)]
pub struct SyncView {
    left_root: VfsPath,
    right_root: VfsPath,
    /// Which way one way and mirror go: from the panel with focus.
    one_way: Direction,
    direction: Direction,
    mirror: bool,
    contents: bool,
    show_same: bool,
    state: ScanState,
    rows: Vec<Row>,
    /// Per row: whether it takes part in the run.
    marked: Vec<bool>,
    unreadable: Vec<String>,
    /// Index into the shown rows.
    cursor: usize,
    /// The first shown row drawn.
    scroll: usize,
    /// How many rows a panel has room for, set by the renderer's measure.
    view_rows: usize,
    /// What has been typed for quick search.
    search: String,
    rescan: bool,
    /// What the two sides are called on screen.
    titles: Option<(String, String)>,
}

impl SyncView {
    /// A view of `left_root` against `right_root`, planned one way in
    /// `direction`, before the scan has reported.
    pub const fn new(left_root: VfsPath, right_root: VfsPath, direction: Direction) -> Self {
        Self {
            left_root,
            right_root,
            one_way: direction,
            direction,
            mirror: false,
            contents: false,
            show_same: false,
            state: ScanState::Scanning { left: 0, right: 0 },
            rows: Vec::new(),
            marked: Vec::new(),
            unreadable: Vec::new(),
            cursor: 0,
            scroll: 0,
            view_rows: 0,
            search: String::new(),
            rescan: false,
            titles: None,
        }
    }

    /// The two roots.
    pub const fn roots(&self) -> (&VfsPath, &VfsPath) {
        (&self.left_root, &self.right_root)
    }

    /// The direction the rows are planned for.
    pub const fn direction(&self) -> Direction {
        self.direction
    }

    /// Whether mirror is on.
    pub const fn mirror(&self) -> bool {
        self.mirror
    }

    /// Whether contents are compared too.
    pub const fn contents(&self) -> bool {
        self.contents
    }

    /// Whether identical files are shown.
    pub const fn show_same(&self) -> bool {
        self.show_same
    }

    /// Where the scan is.
    pub const fn state(&self) -> &ScanState {
        &self.state
    }

    /// Every row, shown or not.
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// Folders that could not be listed.
    pub fn unreadable(&self) -> &[String] {
        &self.unreadable
    }

    /// The quick-search text.
    pub fn search(&self) -> &str {
        &self.search
    }

    /// The cursor, as an index into [`SyncView::shown`].
    pub const fn cursor(&self) -> usize {
        self.cursor
    }

    /// The first shown row on screen.
    pub const fn scroll(&self) -> usize {
        self.scroll
    }

    /// Whether row `index` (into [`SyncView::rows`]) takes part in the run.
    pub fn is_marked(&self, index: usize) -> bool {
        self.marked.get(index).copied().unwrap_or(false)
    }

    /// The scan's running counts.
    pub fn set_progress(&mut self, left: u64, right: u64) {
        self.state = ScanState::Scanning { left, right };
    }

    /// The scan's answer. Every row with something to do starts marked.
    pub fn set_rows(&mut self, rows: Vec<Row>, unreadable: Vec<String>) {
        self.rows = rows;
        self.unreadable = unreadable;
        self.state = ScanState::Done;
        self.cursor = 0;
        self.scroll = 0;
        self.replan();
    }

    /// The scan failed.
    pub fn set_failed(&mut self, why: String) {
        self.state = ScanState::Failed(why);
    }

    /// Whether the trees should be scanned again, asked once.
    pub const fn take_rescan(&mut self) -> bool {
        let wanted = self.rescan;
        self.rescan = false;
        wanted
    }

    /// How many rows the panels show, from the renderer's measure.
    pub fn set_view_rows(&mut self, rows: usize) {
        self.view_rows = rows;
        self.keep_cursor_visible();
    }

    /// Every row back to what the direction and mirror say, and marked
    /// exactly where that is to do something. Undoes `Space` and the arrows.
    fn replan(&mut self) {
        for row in &mut self.rows {
            row.plan = default_plan(row.status, self.direction, self.mirror);
        }
        self.marked = self.rows.iter().map(|r| r.plan != Plan::Skip).collect();
    }

    /// Plan for `direction`, unless it already is.
    pub fn plan_for(&mut self, direction: Direction) {
        if self.direction != direction {
            self.direction = direction;
            self.replan();
        }
    }

    /// The mode the rows are planned for.
    pub const fn mode(&self) -> Mode {
        match (self.direction, self.mirror) {
            (Direction::Both, _) => Mode::Both,
            (_, true) => Mode::Mirror,
            (_, false) => Mode::OneWay,
        }
    }

    /// The mode in words, with its direction: `One way ->`.
    pub fn mode_label(&self, ascii: bool) -> String {
        let arrow = match (self.one_way, ascii) {
            (Direction::ToLeft, false) => "\u{2190}",
            (Direction::ToLeft, true) => "<-",
            (_, false) => "\u{2192}",
            (_, true) => "->",
        };
        match self.mode() {
            Mode::OneWay => format!("One way {arrow}"),
            Mode::Both => {
                if ascii {
                    "Both ways <->".to_string()
                } else {
                    "Both ways \u{2194}".to_string()
                }
            }
            Mode::Mirror => format!("Mirror {arrow}"),
        }
    }

    /// `Tab`: the panel with focus is the source of one way and mirror, so
    /// moving to the other panel turns them round. Every row is replanned,
    /// as a mode change does. Both ways has no source and is left alone.
    pub fn follow_source(&mut self, direction: Direction) {
        if self.one_way == direction {
            return;
        }
        self.one_way = direction;
        if self.mode() != Mode::Both {
            self.direction = direction;
            self.replan();
        }
    }

    /// `Alt+M`: one way, then both ways, then mirror, and round again. Each
    /// replans every row, so the arrows show the new mode straight away.
    pub fn cycle_mode(&mut self) {
        let (direction, mirror) = match self.mode() {
            Mode::OneWay => (Direction::Both, false),
            Mode::Both => (self.one_way, true),
            Mode::Mirror => (self.one_way, false),
        };
        self.direction = direction;
        self.mirror = mirror;
        self.replan();
    }

    /// `Alt+C`: compare contents too, which needs a new scan.
    pub fn toggle_contents(&mut self) {
        self.contents = !self.contents;
        self.rescan = true;
        self.rows.clear();
        self.marked.clear();
        self.cursor = 0;
        self.scroll = 0;
        self.state = ScanState::Scanning { left: 0, right: 0 };
    }

    /// The run has started, or moved on: its progress in words.
    pub fn set_running(&mut self, progress: String) {
        self.state = ScanState::Running(progress);
    }

    /// The byte check's progress.
    pub fn set_checking(&mut self, done: usize, total: usize) {
        self.state = ScanState::Checking { done, total };
    }

    /// What each side is called on screen: the panel's own name for it, an
    /// `sftp://` address for a remote, rather than a bare path that reads
    /// the same on two machines.
    pub fn set_titles(&mut self, left: String, right: String) {
        self.titles = Some((left, right));
    }

    /// The title for one side.
    pub fn title(&self, left: bool) -> String {
        match (&self.titles, left) {
            (Some((l, _)), true) => l.clone(),
            (Some((_, r)), false) => r.clone(),
            (None, true) => self.left_root.to_string(),
            (None, false) => self.right_root.to_string(),
        }
    }

    /// Compare the trees again, keeping the mode and the options: after a
    /// run, so the panels show what it did.
    pub fn rescan(&mut self) {
        self.rescan = true;
        self.rows.clear();
        self.marked.clear();
        self.cursor = 0;
        self.scroll = 0;
        self.state = ScanState::Scanning { left: 0, right: 0 };
    }

    /// `=`: show or hide identical files.
    pub fn toggle_same(&mut self) {
        let here = self.current_index();
        self.show_same = !self.show_same;
        self.cursor = here
            .and_then(|index| self.shown().iter().position(|i| *i == index))
            .unwrap_or(0);
        self.clamp();
    }

    /// Indices into [`SyncView::rows`] of the rows on screen.
    pub fn shown(&self) -> Vec<usize> {
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, row)| self.show_same || row.status != Status::Same)
            .map(|(i, _)| i)
            .collect()
    }

    /// The row under the cursor, as an index into [`SyncView::rows`].
    pub fn current_index(&self) -> Option<usize> {
        self.shown().get(self.cursor).copied()
    }

    /// The row under the cursor.
    pub fn current(&self) -> Option<&Row> {
        self.current_index().and_then(|i| self.rows.get(i))
    }

    /// The file under the cursor on one side, if it is there.
    pub fn current_path(&self, left: bool) -> Option<VfsPath> {
        let row = self.current()?;
        let (facts, root) = if left {
            (row.left, &self.left_root)
        } else {
            (row.right, &self.right_root)
        };
        facts.map(|_| under(root, &row.rel))
    }

    fn clamp(&mut self) {
        let count = self.shown().len();
        self.cursor = self.cursor.min(count.saturating_sub(1));
        self.keep_cursor_visible();
    }

    fn keep_cursor_visible(&mut self) {
        let rows = self.view_rows.max(1);
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll.saturating_add(rows) {
            self.scroll = self.cursor.saturating_sub(rows - 1);
        }
    }

    /// Move the cursor to a shown row.
    pub fn move_to(&mut self, cursor: usize) {
        self.cursor = cursor;
        self.clamp();
    }

    /// Move the cursor by `delta` rows, or a page when asked.
    pub fn move_by(&mut self, delta: isize) {
        let to = self.cursor.saturating_add_signed(delta);
        self.move_to(to);
    }

    /// A page, for `PgUp`/`PgDn`.
    pub fn page(&self) -> isize {
        isize::try_from(self.view_rows.max(1)).unwrap_or(1)
    }

    /// The last shown row.
    pub fn last(&self) -> usize {
        self.shown().len().saturating_sub(1)
    }

    /// `Space`/`Ins`: take the row out of the run, or put it back. A row with
    /// nothing planned has nothing to put back; the arrows give it a plan.
    pub fn toggle_mark(&mut self) -> &'static str {
        let Some(index) = self.current_index() else {
            return "";
        };
        let has_plan = self.rows.get(index).is_some_and(|r| r.plan != Plan::Skip);
        let Some(mark) = self.marked.get_mut(index) else {
            return "";
        };
        if !has_plan {
            return "nothing to do for this file: Left or Right picks a direction";
        }
        *mark = !*mark;
        // Said, because a file that differs starts marked: pressing Space to
        // "select" it unmarks it, and that has to be visible.
        if *mark {
            "marked: it will be synchronized"
        } else {
            "unmarked: it will be left alone (Space marks it again)"
        }
    }

    /// `Right`/`Left`: make the other side match this row's side. Copies the
    /// file across where this side has it, deletes it there where only the
    /// other side does, and marks the row.
    pub fn point(&mut self, to_right: bool) {
        let Some(index) = self.current_index() else {
            return;
        };
        let Some(row) = self.rows.get_mut(index) else {
            return;
        };
        let plan = match (to_right, row.left.is_some(), row.right.is_some()) {
            (true, true, _) => Plan::ToRight,
            (true, false, true) => Plan::DeleteRight,
            (false, _, true) => Plan::ToLeft,
            (false, true, false) => Plan::DeleteLeft,
            (_, false, false) => return,
        };
        if !row.choices().contains(&plan) {
            return;
        }
        row.plan = plan;
        if let Some(mark) = self.marked.get_mut(index) {
            *mark = true;
        }
    }

    /// What a row will have done to it: its plan when marked, nothing when not.
    pub fn effective(&self, index: usize) -> Plan {
        match self.rows.get(index) {
            Some(row) if self.is_marked(index) => row.plan,
            _ => Plan::Skip,
        }
    }

    /// Copies to the right, to the left, deletes, and the bytes copied.
    pub fn counts(&self) -> (usize, usize, usize, u64) {
        let mut out = (0, 0, 0, 0_u64);
        for (index, row) in self.rows.iter().enumerate() {
            match self.effective(index) {
                Plan::ToRight => {
                    out.0 += 1;
                    out.3 = out.3.saturating_add(row.left.map_or(0, |f| f.size));
                }
                Plan::ToLeft => {
                    out.1 += 1;
                    out.3 = out.3.saturating_add(row.right.map_or(0, |f| f.size));
                }
                Plan::DeleteLeft | Plan::DeleteRight => out.2 += 1,
                Plan::Skip => {}
            }
        }
        out
    }

    /// What will be done, in words: `copy 3 to the right (2.1 M), trash 1`.
    pub fn summary(&self) -> String {
        let (right, left, deletes, bytes) = self.counts();
        if right + left + deletes == 0 {
            return self.nothing_planned();
        }
        let mut parts = Vec::new();
        if right > 0 {
            parts.push(format!("{right} to the right"));
        }
        if left > 0 {
            parts.push(format!("{left} to the left"));
        }
        let mut out = String::new();
        if !parts.is_empty() {
            let moved = if bytes < 1024 {
                format!("{bytes} B")
            } else {
                human_size(bytes)
            };
            out = format!("copy {} ({moved})", parts.join(", "));
        }
        if deletes > 0 {
            if !out.is_empty() {
                out.push_str(", ");
            }
            out.push_str(&format!("trash {deletes}"));
        }
        out
    }

    /// "Nothing to do", or, when files differ but none is planned, why and
    /// what would plan them - so a difference on screen is never answered
    /// with a bare "nothing to do".
    fn nothing_planned(&self) -> String {
        let mut newer_left = 0_usize;
        let mut newer_right = 0_usize;
        let mut other = 0_usize;
        for (index, row) in self.rows.iter().enumerate() {
            if self.effective(index) != Plan::Skip {
                continue;
            }
            match row.status {
                Status::LeftNewer => newer_left += 1,
                Status::RightNewer => newer_right += 1,
                Status::Differs | Status::Clash => other += 1,
                Status::Same | Status::OnlyLeft | Status::OnlyRight => {}
            }
        }
        let differ = newer_left + newer_right + other;
        if differ == 0 {
            return "nothing to do".to_string();
        }
        let why = match (newer_left, newer_right) {
            (0, n) if n == differ => ", newer on the right".to_string(),
            (n, 0) if n == differ => ", newer on the left".to_string(),
            _ => String::new(),
        };
        // The keys that would plan them are on the right-hand status line.
        format!(
            "nothing marked; {differ} differ{}{why}",
            if differ == 1 { "s" } else { "" }
        )
    }

    /// The copies and deletes the marked rows plan.
    pub fn answer(&self) -> SyncAnswer {
        let mut answer = SyncAnswer::default();
        for (index, row) in self.rows.iter().enumerate() {
            let left = || under(&self.left_root, &row.rel);
            let right = || under(&self.right_root, &row.rel);
            match self.effective(index) {
                Plan::ToRight => answer.copies.push((left(), right())),
                Plan::ToLeft => answer.copies.push((right(), left())),
                Plan::DeleteLeft => answer.deletes.push(left()),
                Plan::DeleteRight => answer.deletes.push(right()),
                Plan::Skip => {}
            }
        }
        answer
    }

    /// Quick search: add a character and go to the first row from the cursor
    /// whose file name starts with what has been typed.
    pub fn search_push(&mut self, c: char) {
        self.search.push(c);
        self.search_from(self.cursor);
    }

    /// Quick search: take the last character off.
    pub fn search_pop(&mut self) {
        self.search.pop();
    }

    /// Clear the quick search. False when there was none to clear.
    pub fn search_clear(&mut self) -> bool {
        let had = !self.search.is_empty();
        self.search.clear();
        had
    }

    fn search_from(&mut self, start: usize) {
        let wanted = self.search.to_lowercase();
        let shown = self.shown();
        let count = shown.len();
        for step in 0..count {
            let at = (start + step) % count.max(1);
            let Some(row) = shown.get(at).and_then(|i| self.rows.get(*i)) else {
                continue;
            };
            let name = row.rel.rsplit('/').next().unwrap_or(&row.rel);
            if name.to_lowercase().starts_with(&wanted) {
                self.move_to(at);
                return;
            }
        }
    }
}

#[cfg(test)]
#[path = "sync_view_tests.rs"]
pub(crate) mod tests;
