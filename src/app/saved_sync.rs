//! Saved syncs: pairs of places `Ctrl+Alt+S` can open again in one step.
//!
//! A pair is two **locations**, each either a local path or a remote address
//! in the quick-connect form (`sftp://thorin@nas.local:22/srv/backup`). An
//! address names a host, never a password: reopening one reconnects with
//! whatever the host book says about that host - a key, the agent, the
//! keyring - exactly as `Ctrl+R` reconnects a dropped panel.
//!
//! Kept in `syncs.toml`, written by the event loop like `hotlist.toml`. A
//! location that is neither (a member of an archive, a search result) cannot
//! be saved, and nothing offers to.

use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::error::{Error, Result};
use crate::input::DialogId;
use crate::panel::Side;
use crate::vfs::{BackendKind, VfsPath};

/// The file, in the configuration directory.
pub const SYNCS_FILE: &str = "syncs.toml";

const HEADER: &str = "\
# Saved syncs: Ctrl+Alt+S lists them. Each is a left and a right location,
# a local path or a remote address such as sftp://user@host:22/dir. Remote
# addresses never hold a password; reopening one signs in the way the host
# book says.
";

/// One saved pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedSync {
    /// What the list calls it; `F2` in the list sets it. Unset, the two
    /// folders' names stand in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The left panel's location.
    pub left: String,
    /// The right panel's location.
    pub right: String,
}

impl SavedSync {
    /// A pair with no name of its own.
    #[must_use]
    pub const fn new(left: String, right: String) -> Self {
        Self {
            name: None,
            left,
            right,
        }
    }

    /// Its name, or the two folders' own names: `projects <-> backup`.
    #[must_use]
    pub fn shown_name(&self, ascii: bool) -> String {
        if let Some(name) = self.name.as_ref().filter(|n| !n.trim().is_empty()) {
            return name.clone();
        }
        let leaf = |location: &str| {
            location
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .filter(|leaf| !leaf.is_empty())
                .unwrap_or(location)
                .to_string()
        };
        let arrow = if ascii { "<->" } else { "\u{2194}" };
        format!("{} {arrow} {}", leaf(&self.left), leaf(&self.right))
    }

    /// The same two places, whatever either is called.
    #[must_use]
    pub fn same_places(&self, other: &Self) -> bool {
        self.left == other.left && self.right == other.right
    }
    /// `left  <->  right`, for a list row and a question.
    #[must_use]
    pub fn label(&self, ascii: bool) -> String {
        let arrow = if ascii { "<->" } else { "\u{2194}" };
        format!("{}  {arrow}  {}", self.left, self.right)
    }
}

/// The file as a whole: an array of tables called `sync`.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct SyncsFile {
    sync: Vec<SavedSync>,
}

/// The saved pairs, and whether the file is behind.
#[derive(Debug, Default)]
pub struct SavedSyncs {
    list: Vec<SavedSync>,
    dirty: bool,
}

impl SavedSyncs {
    /// Every saved pair, in the order they were saved.
    pub fn list(&self) -> &[SavedSync] {
        &self.list
    }

    /// Whether these two places are saved already, under any name.
    pub fn contains(&self, sync: &SavedSync) -> bool {
        self.list.iter().any(|s| s.same_places(sync))
    }

    /// Name the pair at `index`; an empty name goes back to the default.
    pub fn rename(&mut self, index: usize, name: &str) {
        if let Some(sync) = self.list.get_mut(index) {
            let name = name.trim();
            sync.name = (!name.is_empty()).then(|| name.to_string());
            self.dirty = true;
        }
    }

    /// Save a pair, unless it is saved already.
    pub fn add(&mut self, sync: SavedSync) {
        if !self.contains(&sync) {
            self.list.push(sync);
            self.dirty = true;
        }
    }

    /// Forget the pair at `index`.
    pub fn remove(&mut self, index: usize) {
        if index < self.list.len() {
            self.list.remove(index);
            self.dirty = true;
        }
    }
}

/// Parse the file. A table that does not read costs that table, with a
/// warning, and never the file.
pub fn parse(text: &str) -> (Vec<SavedSync>, Vec<String>) {
    match toml::from_str::<SyncsFile>(text) {
        Ok(file) => {
            let list = file
                .sync
                .into_iter()
                .filter(|s| !s.left.trim().is_empty() && !s.right.trim().is_empty())
                .collect();
            (list, Vec::new())
        }
        Err(err) => (
            Vec::new(),
            vec![format!("{SYNCS_FILE}: {err}; no saved syncs loaded")],
        ),
    }
}

/// Render the pairs back to TOML, with the file's header.
///
/// # Errors
///
/// Serialisation failed, which a list of strings does not.
pub fn render(list: &[SavedSync]) -> Result<String> {
    let body = toml::to_string_pretty(&SyncsFile {
        sync: list.to_vec(),
    })
    .map_err(|e| Error::msg(format!("{SYNCS_FILE}: {e}")))?;
    Ok(format!("{HEADER}\n{body}"))
}

/// Read the file. A missing one is no saved syncs, and no warning.
pub fn load() -> (Vec<SavedSync>, Vec<String>) {
    let Ok(dir) = crate::config::config_dir() else {
        return (Vec::new(), Vec::new());
    };
    let path = dir.join(SYNCS_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => (Vec::new(), Vec::new()),
        Err(err) => (Vec::new(), vec![format!("{}: {err}", path.display())]),
    }
}

/// Write the file. The event loop's, on the blocking pool.
///
/// # Errors
///
/// The configuration directory could not be made or written.
pub fn store(list: &[SavedSync]) -> Result<()> {
    let dir = crate::config::config_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
    let path = dir.join(SYNCS_FILE);
    std::fs::write(&path, render(list)?).map_err(|e| Error::io(&path, e))
}

/// Where a saved location points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    /// A local directory.
    Local(std::path::PathBuf),
    /// A remote directory: the host, and the directory in `dir`.
    Remote(crate::remote::Target),
}

/// Read a saved location back.
///
/// # Errors
///
/// An address this program cannot parse.
pub fn parse_location(text: &str) -> std::result::Result<Location, String> {
    if !text.contains("://") {
        return Ok(Location::Local(std::path::PathBuf::from(text)));
    }
    let user = std::env::var("USER").unwrap_or_else(|_| "root".to_string());
    crate::remote::url::parse(text, crate::remote::Protocol::Sftp, &user)
        .map(|parsed| Location::Remote(parsed.target))
        .map_err(|_| format!("{text}: not an address this program can open"))
}

/// The same host, whatever directory either names.
fn same_host(a: &crate::remote::Target, b: &crate::remote::Target) -> bool {
    a.protocol == b.protocol && a.host == b.host && a.port == b.port && a.user == b.user
}

/// Where a saved sync is waiting to open, and since when.
#[derive(Debug, Clone)]
pub struct Opening {
    left: String,
    right: String,
    since: std::time::Instant,
}

/// How long a saved sync waits for its connections before giving up.
const OPEN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

impl App {
    /// Read `syncs.toml` at startup, like the hotlist.
    pub fn load_saved_syncs(&mut self) {
        let (list, warnings) = load();
        self.saved_syncs.list = list;
        self.warnings.extend(warnings);
    }

    /// The pairs as they need writing, or `None` when the file is current.
    pub fn take_saved_syncs_write(&mut self) -> Option<Vec<SavedSync>> {
        if !self.saved_syncs.dirty {
            return None;
        }
        self.saved_syncs.dirty = false;
        Some(self.saved_syncs.list.clone())
    }

    /// The saved pairs.
    pub const fn saved_syncs(&self) -> &SavedSyncs {
        &self.saved_syncs
    }

    /// What a path would be saved as: a local path, or the address of the
    /// connection it is on. `None` for anything nested - inside an archive,
    /// a disk image, a search result - which has no address to reopen.
    pub fn location_of(&self, path: &VfsPath) -> Option<String> {
        if path.segments().len() != 1 {
            return None;
        }
        match path.backend() {
            BackendKind::Local => path.local_path().map(|p| p.to_string_lossy().into_owned()),
            BackendKind::Remote(id) => {
                let fs = self.router.remotes().get(id)?;
                Some(fs.target().url(&path.tail().to_string_lossy()))
            }
            BackendKind::List
            | BackendKind::Archive
            | BackendKind::Image
            | BackendKind::Sqlite
            | BackendKind::Git => None,
        }
    }

    /// The pair the two panels are showing, if both can be saved.
    pub fn current_pair(&self) -> Option<SavedSync> {
        Some(SavedSync::new(
            self.location_of(&self.left.active_tab().path)?,
            self.location_of(&self.right.active_tab().path)?,
        ))
    }

    /// After a run: offer to save the pair, once, if it can be and is not.
    pub(crate) fn offer_to_remember(&mut self) {
        let Some(pair) = self.current_pair() else {
            return;
        };
        if self.saved_syncs.contains(&pair) {
            return;
        }
        let ascii = self.config.ui.ascii_borders;
        let dialog = crate::dialog::ConfirmDialog::new(
            DialogId::SyncRemember,
            "Remember this sync?",
            vec![
                pair.label(ascii),
                String::new(),
                "Ctrl+Alt+S lists it from now on.".to_string(),
            ],
        )
        .with_buttons("Remember", "Not now");
        self.sync_remember = Some(pair);
        self.push_dialog(Box::new(dialog));
    }

    /// The question answered.
    pub fn answer_remember(&mut self, yes: bool) {
        let Some(pair) = self.sync_remember.take() else {
            return;
        };
        if yes {
            self.saved_syncs.add(pair);
            self.message = Some("sync remembered; Ctrl+Alt+S lists it".to_string());
        }
    }

    /// `Ctrl+Alt+S`: with nothing saved, compare the panels straight away;
    /// otherwise offer the panels and every saved pair.
    pub fn open_sync(&mut self) {
        if self.saved_syncs.list.is_empty() {
            self.request_sync();
            return;
        }
        use crate::ui::dialog::syncpick::{Choice, SyncPicker};
        let ascii = self.config.ui.ascii_borders;
        let (left, right) = self
            .current_pair()
            .map_or_else(|| (String::new(), String::new()), |p| (p.left, p.right));
        let here = Choice {
            name: "Compare these panels".to_string(),
            left,
            right,
        };
        let saved = self
            .saved_syncs
            .list
            .iter()
            .map(|s| Choice {
                name: s.shown_name(ascii),
                left: s.left.clone(),
                right: s.right.clone(),
            })
            .collect();
        self.push_dialog(Box::new(SyncPicker::new(here, saved)));
    }

    /// The picker's answer.
    pub fn answer_sync_pick(&mut self, pick: crate::ui::dialog::syncpick::Pick) {
        use crate::ui::dialog::syncpick::Pick;
        match pick {
            Pick::Panels => self.request_sync(),
            Pick::Open(index) => {
                if let Some(pair) = self.saved_syncs.list.get(index).cloned() {
                    self.open_saved(&pair);
                }
            }
            Pick::Forget(index) => self.saved_syncs.remove(index),
            Pick::Rename(index) => {
                let Some(pair) = self.saved_syncs.list.get(index) else {
                    return;
                };
                let current = pair.shown_name(self.config.ui.ascii_borders);
                self.sync_renaming = Some(index);
                self.push_dialog(Box::new(
                    crate::dialog::InputDialog::new(
                        DialogId::SyncRename,
                        "Rename sync",
                        "Name:",
                        current,
                    )
                    .allowing_empty(),
                ));
            }
        }
    }

    /// The rename prompt's answer: name the pair, and the list under it.
    pub fn answer_sync_rename(&mut self, name: &str) {
        let Some(index) = self.sync_renaming.take() else {
            return;
        };
        self.saved_syncs.rename(index, name);
        let shown = self
            .saved_syncs
            .list
            .get(index)
            .map(|s| s.shown_name(self.config.ui.ascii_borders));
        if let Some(shown) = shown
            && let Some(picker) = self
                .dialogs_mut()
                .iter_mut()
                .find(|frame| frame.dialog.id() == DialogId::SyncPicker)
                .and_then(|frame| frame.dialog.as_any_mut())
                .and_then(|any| any.downcast_mut::<crate::ui::dialog::syncpick::SyncPicker>())
        {
            picker.set_name(index, shown);
        }
    }

    /// Put each panel where the pair says, connecting where needed, and start
    /// the compare once both have arrived ([`App::service_saved_open`]).
    fn open_saved(&mut self, pair: &SavedSync) {
        for (side, text) in [(Side::Left, &pair.left), (Side::Right, &pair.right)] {
            match parse_location(text) {
                Ok(Location::Local(path)) => self.navigate(side, VfsPath::local(path)),
                Ok(Location::Remote(target)) => self.open_remote_on(side, target),
                Err(why) => {
                    self.message = Some(why);
                    return;
                }
            }
        }
        self.sync_opening = Some(Opening {
            left: pair.left.clone(),
            right: pair.right.clone(),
            since: std::time::Instant::now(),
        });
        self.message = Some("opening the saved sync...".to_string());
    }

    /// A remote side: an open connection to the host if there is one, or a
    /// new one signed in the way the host book says.
    fn open_remote_on(&mut self, side: Side, target: crate::remote::Target) {
        let dir = target.dir.clone().unwrap_or_else(|| "/".to_string());
        let live = self.router.remotes().ids().into_iter().find(|id| {
            self.router
                .remotes()
                .get(*id)
                .is_some_and(|fs| !fs.is_lost() && same_host(fs.target(), &target))
        });
        if let Some(id) = live {
            self.navigate(side, id.path(&dir));
            return;
        }
        self.connect_side(side, target);
    }

    /// Start the compare once both panels show the saved pair. Gives up,
    /// saying so, if a connection never arrives.
    pub fn service_saved_open(&mut self) {
        let Some(opening) = self.sync_opening.clone() else {
            return;
        };
        let arrived = |app: &Self, side: Side, want: &str| {
            let tab = app.panel(side).active_tab();
            !tab.loading && app.location_of(&tab.path).as_deref() == Some(want)
        };
        if arrived(self, Side::Left, &opening.left) && arrived(self, Side::Right, &opening.right) {
            self.sync_opening = None;
            self.request_sync();
            return;
        }
        if opening.since.elapsed() >= OPEN_TIMEOUT {
            self.sync_opening = None;
            self.message = Some("the saved sync did not open: a side never arrived".to_string());
        }
    }
}

#[cfg(test)]
#[path = "saved_sync_tests.rs"]
mod tests;
