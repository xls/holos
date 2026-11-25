//! Keys while the sync view is up (`Ctrl+Alt+S`).
//!
//! The panels are showing the view, so a panel key means the view's version
//! of it: the cursor keys move the one cursor both panels share, `Space` and
//! `Ins` unmark or re-mark a row, `F5` runs what the panels show (after
//! asking), `F3` views the file on the active side. A few keys are the view's
//! own: `Left` and `Right` point a row, `=` shows identical files, `Alt+M`
//! (or `Shift+F5`) steps the mode - one way, both ways, mirror - and `Alt+C`
//! turns the byte check on and off: files whose size and date match are read
//! and compared as well. `Tab` moves to the other side, which
//! makes it the source of one way and mirror. Typing is quick
//! search. Keys that open something over the panels - help, the menu, the
//! console, the job queue - work as they do anywhere; the rest are swallowed,
//! because a panel operation on a listing that is not a directory would act
//! on something nobody can see.

use super::{Action, KeyCode, KeyContext, KeyModifiers, KeyPress, Resolution, run_action};
use crate::Result;
use crate::app::App;
use crate::panel::Side;

/// Route one key while the sync view is up.
pub(super) fn sync_key(app: &mut App, press: KeyPress) -> Result<()> {
    let ctrl = press.mods.contains(KeyModifiers::CONTROL);
    let alt = press.mods.contains(KeyModifiers::ALT);
    let plain = !ctrl && !alt;
    let Some(view) = app.sync_view_mut() else {
        return Ok(());
    };
    match press.code {
        KeyCode::Char('m' | 'M') if alt && !ctrl => {
            view.cycle_mode();
            return Ok(());
        }
        KeyCode::Char('c' | 'C') if alt && !ctrl => {
            view.toggle_contents();
            return Ok(());
        }
        KeyCode::Char('=') if plain => {
            view.toggle_same();
            return Ok(());
        }
        KeyCode::Left if press.mods.is_empty() => {
            view.point(false);
            return Ok(());
        }
        KeyCode::Right if press.mods.is_empty() => {
            view.point(true);
            return Ok(());
        }
        KeyCode::Backspace if !view.search().is_empty() => {
            view.search_pop();
            return Ok(());
        }
        _ => {}
    }
    match app.keymap.resolve(KeyContext::Panel, press) {
        Resolution::Action(action) => sync_action(app, action, press),
        Resolution::ChordPending => Ok(()),
        Resolution::Unbound => {
            if let Some(c) = press.as_text()
                && let Some(view) = app.sync_view_mut()
            {
                view.search_push(c);
            }
            Ok(())
        }
    }
}

fn sync_action(app: &mut App, action: Action, press: KeyPress) -> Result<()> {
    let left = app.active_side == Side::Left;
    let Some(view) = app.sync_view_mut() else {
        return Ok(());
    };
    let page = view.page();
    match action {
        Action::CursorUp => view.move_by(-1),
        Action::CursorDown => view.move_by(1),
        Action::CursorPageUp => view.move_by(-page),
        Action::CursorPageDown => view.move_by(page),
        Action::CursorTop => view.move_to(0),
        Action::CursorBottom => {
            let last = view.last();
            view.move_to(last);
        }
        Action::ToggleSelect => {
            view.toggle_mark();
            view.move_by(1);
        }
        Action::SelectAndSize => view.toggle_mark(),
        Action::ClearSearch => {
            if !view.search_clear() {
                app.leave_sync();
            }
        }
        Action::Copy => app.ask_sync(),
        // `Shift+F5` steps the mode too: shift is what makes a run go both
        // ways, and the arrows say so before `F5` runs it.
        Action::CopySameDir => view.cycle_mode(),
        Action::View => match view.current_path(left) {
            Some(path) => app.request_view(crate::app::ViewRequest::File { path, at: None }),
            None => app.message = Some("this side has no such file".to_string()),
        },
        Action::SyncDirs => app.leave_sync(),
        Action::OtherPanel => {
            run_action(app, action, press)?;
            let source = app.one_way();
            if let Some(view) = app.sync_view_mut() {
                view.follow_source(source);
            }
        }
        Action::Help
        | Action::Quit
        | Action::Menu
        | Action::MenuFiles
        | Action::MenuMark
        | Action::MenuCommands
        | Action::MenuNet
        | Action::MenuShow
        | Action::MenuConfig
        | Action::ConsoleToggle
        | Action::JobQueue => return run_action(app, action, press),
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;
