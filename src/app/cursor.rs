//! Where the selection is: the cursor, and the marks.
//!
//! The cursor is always on an existing row or on none, which is what every
//! move here clamps for, and moving it clears the quick-search buffer.
//! The one exception is [`App::move_cursor_to_match`], which
//! is the search itself and would otherwise erase the query that found the
//! row.
//!
//! # Marks are the tab's, keyed by where the row really lives
//!
//! The `..` row is never marked. Everything else is delegated to
//! [`crate::panel::Tab`], and that delegation is the point rather than a
//! formality: `App` used to keep its own copy of all three mark operations
//! keyed on the row's displayed name, while the tab reads them by
//! [`crate::vfs::Entry::mark_key`]. On an ordinary listing the two strings
//! agree and nothing showed. On a virtual one the key is the
//! row's real location, so a mark was written under a name nothing ever
//! looked up: marking a search hit did nothing at all, and `F5` and `F8` then
//! found no operands.

use crate::app::App;

impl App {
    /// Move the cursor by a signed number of rows, clamping.
    ///
    /// Cursor movement clears the quick-search buffer.
    pub fn move_cursor(&mut self, delta: isize) {
        // In filter mode the arrows walk what the filter left, so the search is
        // kept rather than cleared; `Esc` is what ends it. Everywhere else a
        // cursor move ends the quick search, as it always has.
        let keep_filter = self.config.panel.quick_search_filter
            && self.active_panel().active_tab().is_quick_filtered();
        let panel = self.active_panel_mut();
        if !keep_filter {
            panel.quick.clear();
        }
        let rows = panel.view_rows;
        // `move_by` walks shown rows only when a filter is hiding some, and is
        // the ordinary index step otherwise; it also clamps and scrolls.
        panel.active_tab_mut().move_by(delta, rows);
        // the quick view follows the active panel's cursor.
        // Here rather than in the key handlers, because every key that moves a
        // cursor comes through this one function and a debounce armed in only
        // some of them would be a quick view that follows some keys.
        self.note_quick_view_cursor();
    }

    /// Sweep the mark from the cursor to `index`, and land there.
    ///
    /// `Shift` with a movement key, the way Total Commander does it: the row
    /// the sweep starts on decides whether the run marks or clears, so a
    /// `Shift+End` on a clean listing selects to the bottom and a second one
    /// gives it back. See [`crate::panel::Tab::mark_through`].
    pub fn mark_cursor_through(&mut self, index: usize) {
        let panel = self.active_panel_mut();
        panel.quick.clear();
        let rows = panel.view_rows;
        panel.active_tab_mut().mark_through(index, rows);
        self.note_quick_view_cursor();
    }

    /// Put the cursor on a row, clamping. Clears the quick-search buffer.
    pub fn move_cursor_to(&mut self, index: usize) {
        let keep_filter = self.config.panel.quick_search_filter
            && self.active_panel().active_tab().is_quick_filtered();
        let panel = self.active_panel_mut();
        if !keep_filter {
            panel.quick.clear();
        }
        let rows = panel.view_rows;
        panel.active_tab_mut().move_to(index, rows);
        self.note_quick_view_cursor();
    }

    /// Move the cursor to the first entry matching a quick-search query.
    /// Returns false when nothing matched, in which case the
    /// cursor does not move and the buffer is kept.
    ///
    /// Unlike [`App::move_cursor`] this does **not** clear the buffer.
    pub fn move_cursor_to_match(&mut self, query: &str) -> bool {
        let mode = self.config.panel.quick_search;
        let case = self.config.panel.quick_search_case;
        let panel = self.active_panel_mut();
        let found = panel.active_tab().entries.iter().position(|e| {
            !e.is_parent && crate::input::quicksearch::quick_match(&e.name, query, mode, case)
        });
        let Some(index) = found else { return false };
        panel.active_tab_mut().cursor = index;
        let rows = panel.view_rows;
        panel.active_tab_mut().scroll_into_view(rows);
        // A quick search moves the cursor too, so the quick view follows it.
        //
        self.note_quick_view_cursor();
        true
    }

    /// `Up` / `Down` while a quick search is typed: the previous or next row
    /// that matches it, keeping the search.
    ///
    /// Without this an arrow ended the search and stepped one row, so of
    /// several names matching what was typed only the first could ever be
    /// reached - a folder that sorted first kept the cursor away from every
    /// file sharing its prefix. Returns false when no search is on, and the
    /// caller then moves the cursor the ordinary way. Past the last match it
    /// goes round to the first, and before the first to the last.
    ///
    /// Filter mode needs nothing of its own: the filter shows exactly the
    /// rows this matcher accepts, so walking the matches is walking what is
    /// on screen, and it goes round the same way.
    pub fn step_quick_match(&mut self, forward: bool) -> bool {
        if self.active_panel().quick.is_empty() {
            return false;
        }
        let mode = self.config.panel.quick_search;
        let case = self.config.panel.quick_search_case;
        let query = self.active_panel().quick.buffer.clone();
        let panel = self.active_panel_mut();
        let rows = panel.view_rows;
        panel
            .active_tab_mut()
            .step_wrapping(forward, rows, |entry| {
                crate::input::quicksearch::quick_match(&entry.name, &query, mode, case)
            });
        self.note_quick_view_cursor();
        true
    }

    /// `Insert` / `Space`: toggle the mark under the cursor.
    /// The `..` row never marks.
    /// Delegated, and that is the whole point: these three used to key marks
    /// on `Entry::name` while [`crate::panel::Tab::is_marked`] reads them by
    /// [`crate::vfs::Entry::mark_key`]. On an ordinary listing the two are the
    /// same string and nothing showed; on a virtual one the
    /// key is the row's real location, so every mark was written under a name
    /// nothing ever looked up. Marking a search hit did nothing at all, and
    /// `F5` and `F8` then found no operands. One implementation, in the type
    /// that owns the set.
    pub fn toggle_mark_under_cursor(&mut self) {
        self.active_panel_mut().active_tab_mut().toggle_mark();
    }

    /// `*`: invert the marks. See [`App::toggle_mark_under_cursor`].
    pub fn invert_marks(&mut self) {
        self.active_panel_mut().active_tab_mut().invert_marks();
    }

    /// `Ctrl+A`: mark everything. See [`App::toggle_mark_under_cursor`].
    pub fn mark_all(&mut self) {
        self.active_panel_mut().active_tab_mut().mark_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::app_with;
    use crate::config::{Config, Keymap, Theme};
    use crate::vfs::{Entry, VfsPath};

    #[test]
    fn the_arrows_walk_the_quick_search_matches_and_keep_the_search() {
        use crate::input::{KeyCode, KeyEvent, KeyModifiers, dispatch};
        // The bug: `d` found `docs`, and Down then ended the search and
        // stepped onto `alpha` - the two files matching `d` were unreachable.
        let mut app = app_with(&["docs", "alpha", "data.csv", "dump.sql", "zeta"]);
        let press = |app: &mut App, code: KeyCode| {
            dispatch(app, KeyEvent::new(code, KeyModifiers::NONE)).expect("dispatch");
        };
        let cursor = |app: &App| app.left.active_tab().cursor;
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(cursor(&app), 0, "the first match");
        press(&mut app, KeyCode::Down);
        assert_eq!(cursor(&app), 2, "the next match, skipping alpha");
        press(&mut app, KeyCode::Down);
        assert_eq!(cursor(&app), 3);
        press(&mut app, KeyCode::Down);
        assert_eq!(cursor(&app), 0, "past the last match it goes round");
        assert_eq!(app.left.quick.buffer, "d", "and the search is kept");
        press(&mut app, KeyCode::Up);
        assert_eq!(cursor(&app), 3, "before the first, round to the last");
        press(&mut app, KeyCode::Up);
        assert_eq!(cursor(&app), 2, "Up walks back through the matches");
        // Esc ends the search; the arrows are ordinary again.
        press(&mut app, KeyCode::Esc);
        assert!(app.left.quick.is_empty());
        press(&mut app, KeyCode::Down);
        assert_eq!(cursor(&app), 3, "one row");
    }

    #[test]
    fn in_filter_mode_the_arrows_go_round_the_rows_the_filter_shows() {
        use crate::input::{KeyCode, KeyEvent, KeyModifiers, dispatch};
        let mut app = app_with(&["..", "docs", "alpha", "data.csv", "dump.sql", "zeta"]);
        if let Some(parent) = app.left.active_tab_mut().entries.first_mut() {
            parent.is_parent = true;
        }
        app.config.panel.quick_search_filter = true;
        let press = |app: &mut App, code: KeyCode| {
            dispatch(app, KeyEvent::new(code, KeyModifiers::NONE)).expect("dispatch");
        };
        let name = |app: &App| app.left.active_tab().cursor_name().unwrap_or_default();
        press(&mut app, KeyCode::Char('d'));
        assert!(app.left.active_tab().is_quick_filtered());
        assert_eq!(name(&app), "docs", "the first match");
        press(&mut app, KeyCode::Up);
        assert_eq!(
            name(&app),
            "dump.sql",
            "Up at the top goes round to the bottom"
        );
        press(&mut app, KeyCode::Down);
        assert_eq!(name(&app), "docs", "and Down at the bottom back to the top");
        press(&mut app, KeyCode::Down);
        assert_eq!(name(&app), "data.csv", "skipping what the filter hides");
        assert_eq!(app.left.quick.buffer, "d", "the filter is kept");
    }

    #[test]
    fn quick_search_moves_the_cursor_to_the_first_match() {
        let mut app = app_with(&["alpha", "Thorin", "thorin", "zeta"]);
        assert!(app.move_cursor_to_match("tho"));
        assert_eq!(app.left.active_tab().cursor, 1, "Thorin comes first");
        assert!(app.move_cursor_to_match("Tho"));
        assert_eq!(app.left.active_tab().cursor, 1);
        assert!(!app.move_cursor_to_match("zzz"));
        assert_eq!(app.left.active_tab().cursor, 1, "a miss does not move");
    }

    #[test]
    fn cursor_movement_clears_the_quick_search_buffer() {
        let mut app = app_with(&["a", "b"]);
        app.left.quick.push('a');
        app.move_cursor(1);
        assert!(app.left.quick.is_empty());
    }

    #[test]
    fn marks_never_include_the_parent_row() {
        let mut app = App::headless(Config::default(), Keymap::builtin(), Theme::blue());
        app.left.active_tab_mut().entries =
            vec![Entry::parent_entry(), Entry::file("a"), Entry::file("b")];
        app.toggle_mark_under_cursor();
        assert!(app.left.active_tab().marks.is_empty());
        app.mark_all();
        assert_eq!(app.left.active_tab().marks.len(), 2);
        app.invert_marks();
        assert!(app.left.active_tab().marks.is_empty());
    }

    #[test]
    fn marking_a_search_hit_goes_through_the_tab_and_keys_on_its_real_home() {
        // The bug this pins: `App` had its own copy of all three mark
        // functions, keyed on `Entry::name`, while `Tab::is_marked` reads by
        // `Entry::mark_key`. On a real directory the two are the same string
        // and everything worked. On a search listing the key
        // is the row's real location, so `Insert` wrote a mark under a name
        // nothing looked up: the row never highlighted, the status line never
        // counted it, and `F5` and `F8` found no operands at all. Verified on
        // the binary before and after.
        use crate::vfs::{BackendKind, Entry};
        let mut app = App::headless(Config::default(), Keymap::builtin(), Theme::blue());
        let tab = app.left.active_tab_mut();
        tab.path = VfsPath::new(BackendKind::List, "/1");
        tab.entries = ["/root/one/mod.rs", "/root/two/mod.rs"]
            .iter()
            .map(|home| {
                let path = VfsPath::local(*home);
                let mut entry = Entry::file("mod.rs".to_string());
                entry.location = Some(path);
                entry
            })
            .collect();
        tab.cursor = 0;

        app.toggle_mark_under_cursor();
        let tab = app.left.active_tab();
        assert!(tab.is_marked(&tab.entries[0]), "the row under the cursor");
        assert!(
            !tab.is_marked(&tab.entries[1]),
            "and not its namesake in another directory"
        );
        assert_eq!(
            tab.operand_paths(),
            vec![VfsPath::local("/root/one/mod.rs")]
        );

        // `Ctrl+A` and `*` had the same duplicated key and are delegated too.
        app.mark_all();
        assert_eq!(app.left.active_tab().operand_paths().len(), 2);
        // Inverting a fully marked listing clears it. Asserted on the mark
        // set rather than on `operand_paths`, which falls back to the row
        // under the cursor when nothing is marked.
        app.invert_marks();
        let tab = app.left.active_tab();
        assert!(!tab.is_marked(&tab.entries[0]));
        assert!(!tab.is_marked(&tab.entries[1]));
    }
}
