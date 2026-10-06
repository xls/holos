//! Restarting into the new version once a self-update has replaced the binary.
//!
//! The update itself is `npx holos-installer` typed into the console, so hcmd
//! does not see its exit code. What it can see is its own executable: the
//! installer writes the new binary under a temporary name and renames it over
//! the old one, so the file at hcmd's path changes in one step - a different
//! inode, size or modification time - and is complete the moment it does.
//!
//! So accepting the update arms a watch on that path, the event loop looks at
//! it once a second while it is armed, and when it changes the user is asked
//! whether to restart now. "Later" keeps the running version until they quit.
//! Restarting is an ordinary quit - tabs saved, terminal restored - after
//! which `main` replaces the process with the new binary, same arguments, same
//! terminal. Nothing restarts without the question: a restart ends whatever is
//! running in the console.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use crate::app::App;
use crate::dialog::ConfirmDialog;
use crate::input::DialogId;

/// How often an armed watch looks at the file.
const POLL: Duration = Duration::from_secs(1);

/// The executable as it was when the watch was armed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExeStamp {
    /// Size in bytes.
    len: u64,
    /// Last modified.
    modified: Option<SystemTime>,
    /// The inode, which a rename over the file always changes.
    inode: u64,
}

impl ExeStamp {
    /// The stamp of the file at `path`, or `None` when it cannot be read.
    pub fn of(path: &Path) -> Option<Self> {
        use std::os::unix::fs::MetadataExt as _;
        let meta = std::fs::metadata(path).ok()?;
        Some(Self {
            len: meta.len(),
            modified: meta.modified().ok(),
            inode: meta.ino(),
        })
    }
}

/// A watch on the running binary, armed by accepting a self-update.
#[derive(Debug, Clone)]
pub struct ExeWatch {
    /// Where the running binary lives - taken before it is replaced, since
    /// afterwards the running process's own path names a deleted file.
    path: PathBuf,
    /// What it looked like when the update started.
    before: ExeStamp,
    /// When it was last looked at.
    checked: Instant,
}

impl ExeWatch {
    /// A watch on `path` as it is now.
    pub fn arm(path: PathBuf, now: Instant) -> Option<Self> {
        let before = ExeStamp::of(&path)?;
        Some(Self {
            path,
            before,
            checked: now,
        })
    }

    /// Whether the file has been replaced. Looks at most once per [`POLL`];
    /// a file that cannot be read right now is not a replacement.
    pub fn replaced(&mut self, now: Instant) -> bool {
        if now.saturating_duration_since(self.checked) < POLL {
            return false;
        }
        self.checked = now;
        ExeStamp::of(&self.path).is_some_and(|stamp| stamp != self.before)
    }

    /// The binary to start in place of this one.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl App {
    /// Start watching the running binary for the self-update to replace it.
    pub fn watch_for_new_binary(&mut self, now: Instant) {
        self.exe_watch = std::env::current_exe()
            .ok()
            .and_then(|path| ExeWatch::arm(path, now));
    }

    /// The event loop's half: when the watched binary has been replaced, stop
    /// watching and ask whether to restart into it.
    pub fn service_exe_watch(&mut self, now: Instant) {
        let Some(watch) = self.exe_watch.as_mut() else {
            return;
        };
        if !watch.replaced(now) {
            return;
        }
        self.restart_into = Some(watch.path().to_path_buf());
        self.exe_watch = None;
        self.push_dialog(Box::new(
            ConfirmDialog::new(
                DialogId::Restart,
                "Updated",
                vec![
                    "hcmd was updated. Restart now to run the new version?".to_string(),
                    "Your tabs are kept; anything running in the console ends.".to_string(),
                ],
            )
            .with_buttons("Restart", "Later"),
        ));
    }

    /// The answer: restart now, or keep running the old version until quit.
    pub fn answer_restart(&mut self, now: bool) {
        if now {
            self.restart_requested = true;
            self.should_quit = true;
        } else {
            self.restart_into = None;
            self.message = Some("the new version starts the next time hcmd does".to_string());
        }
    }

    /// The binary `main` should start once the loop has ended, if a restart
    /// was asked for.
    #[must_use]
    pub fn restart_target(&self) -> Option<&Path> {
        self.restart_requested
            .then_some(self.restart_into.as_deref())
            .flatten()
    }
}

#[cfg(test)]
#[path = "restart_tests.rs"]
mod tests;
