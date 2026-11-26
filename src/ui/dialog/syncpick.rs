//! `Ctrl+Alt+S` once something is saved: compare these panels, or open a
//! saved sync.
//!
//! ```text
//! ┌──────────────────── Synchronize ─────────────────────┐
//! │ Search: pro                                          │
//! │ Compare these panels                                 │
//! │   /home/t/projects  <->  /mnt/usb/projects           │
//! │ projects <-> backup                                  │
//! │   /home/t/projects  <->  sftp://t@nas:22/b...p/proj  │
//! └────────── Enter opens  F2 renames  Del forgets ──────┘
//! ```
//!
//! Each choice is two rows: its name, and its two sides shortened in the
//! middle so the start and the end of both stay in view. Typing filters by
//! name or path; the list scrolls with the cursor.
//!
//! Answers, for [`read_answer`]: `Text("")` for the panels, `Text("<n>")` to
//! open saved pair `n`; and, staying open, `Act(Text("forget:<n>"))` for
//! `Del` and `Act(Text("rename:<n>"))` for `F2`.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::row;
use crate::dialog::{Dialog, DialogKey, DialogOutcome, DialogResult, DialogStyle};
use crate::input::{DialogId, KeyCode};
use crate::ui::text;

/// One choice: a name and its two sides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// What the list calls it.
    pub name: String,
    /// The left location.
    pub left: String,
    /// The right location.
    pub right: String,
}

/// What the picker's answer means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    /// Compare the panels as they are.
    Panels,
    /// Open saved pair `n`.
    Open(usize),
    /// Forget saved pair `n`.
    Forget(usize),
    /// Rename saved pair `n`.
    Rename(usize),
}

/// Read the picker's answer back.
#[must_use]
pub fn read_answer(text: &str) -> Pick {
    let index = |n: &str| n.parse::<usize>().ok();
    if let Some(n) = text.strip_prefix("forget:").and_then(index) {
        return Pick::Forget(n);
    }
    if let Some(n) = text.strip_prefix("rename:").and_then(index) {
        return Pick::Rename(n);
    }
    index(text).map_or(Pick::Panels, Pick::Open)
}

/// The picker.
#[derive(Debug)]
pub struct SyncPicker {
    here: Choice,
    saved: Vec<Choice>,
    search: String,
    /// Index into [`SyncPicker::shown`]; 0 is the panels.
    cursor: usize,
    /// The first saved choice drawn: the panels are pinned above the
    /// separator, and only the saved list scrolls.
    top: usize,
    /// Saved choices that fit below the separator, from the last layout.
    fits: usize,
}

impl SyncPicker {
    /// A picker offering the panels and the saved pairs.
    pub const fn new(here: Choice, saved: Vec<Choice>) -> Self {
        Self {
            here,
            saved,
            search: String::new(),
            cursor: 0,
            top: 0,
            fits: 8,
        }
    }

    /// The saved choices still listed.
    pub fn saved(&self) -> &[Choice] {
        &self.saved
    }

    /// Rename saved choice `n` on screen, after the app has renamed it.
    pub fn set_name(&mut self, n: usize, name: String) {
        if let Some(choice) = self.saved.get_mut(n) {
            choice.name = name;
        }
    }

    /// What is typed.
    pub fn search(&self) -> &str {
        &self.search
    }

    /// The choices on screen: `None` for the panels, `Some(n)` for saved pair
    /// `n`. The panels are always first and never filtered out.
    pub fn shown(&self) -> Vec<Option<usize>> {
        let wanted = self.search.to_lowercase();
        std::iter::once(None)
            .chain(
                self.saved
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| {
                        wanted.is_empty()
                            || [&c.name, &c.left, &c.right]
                                .iter()
                                .any(|s| s.to_lowercase().contains(&wanted))
                    })
                    .map(|(n, _)| Some(n)),
            )
            .collect()
    }

    fn current(&self) -> Option<usize> {
        self.shown().get(self.cursor).copied().flatten()
    }

    fn move_to(&mut self, cursor: usize) {
        let last = self.shown().len().saturating_sub(1);
        self.cursor = cursor.min(last);
        // Position among the saved choices, which start at shown index 1.
        let Some(saved_at) = self.cursor.checked_sub(1) else {
            return;
        };
        let fits = self.fits.max(1);
        if saved_at < self.top {
            self.top = saved_at;
        } else if saved_at >= self.top + fits {
            self.top = saved_at + 1 - fits;
        }
    }

    fn choice(&self, which: Option<usize>) -> Option<&Choice> {
        match which {
            None => Some(&self.here),
            Some(n) => self.saved.get(n),
        }
    }
}

/// `text` shortened to `width` by cutting its middle, so both ends show.
fn middle(text_in: &str, width: usize, ascii: bool) -> String {
    text::fit_left(text_in, width, text::Crop::Middle, super::ellipsis(ascii))
        .trim_end()
        .to_string()
}

impl Dialog for SyncPicker {
    fn id(&self) -> DialogId {
        DialogId::SyncPicker
    }

    fn title(&self) -> String {
        "Synchronize".to_string()
    }

    fn size_hint(&self) -> (u16, u16) {
        let rows = self
            .saved
            .len()
            .saturating_add(1)
            .saturating_mul(2)
            .saturating_add(4);
        (110, u16::try_from(rows).unwrap_or(u16::MAX).min(32))
    }

    fn footer(&self) -> Option<String> {
        Some("Enter opens  F2 renames  Del forgets".to_string())
    }

    /// None: typing is the search.
    fn mnemonic_letters(&self) -> Vec<char> {
        Vec::new()
    }

    /// The app reaches in to show a new name after `F2`.
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }

    fn layout(&mut self, area: Rect) {
        // The search line, the panels' two rows and the separator, then two
        // rows a saved choice.
        self.fits = usize::from(area.height.saturating_sub(4)) / 2;
        self.move_to(self.cursor);
    }

    fn handle_key(&mut self, key: &DialogKey) -> DialogOutcome {
        if key.is_cancel() {
            if self.search.is_empty() {
                return DialogOutcome::Cancel;
            }
            self.search.clear();
            self.move_to(0);
            return DialogOutcome::Consumed;
        }
        if key.is_accept() {
            return DialogOutcome::Accept(DialogResult::Text(
                self.current().map_or_else(String::new, |n| n.to_string()),
            ));
        }
        let last = self.shown().len().saturating_sub(1);
        match key.press.code {
            KeyCode::Up => self.move_to(self.cursor.saturating_sub(1)),
            KeyCode::Down => self.move_to(self.cursor.saturating_add(1)),
            KeyCode::PageUp => self.move_to(self.cursor.saturating_sub(self.fits.max(1))),
            KeyCode::PageDown => self.move_to(self.cursor.saturating_add(self.fits.max(1))),
            KeyCode::Home => self.move_to(0),
            KeyCode::End => self.move_to(last),
            KeyCode::F(2) => {
                return self.current().map_or(DialogOutcome::Consumed, |n| {
                    DialogOutcome::Act(DialogResult::Text(format!("rename:{n}")))
                });
            }
            KeyCode::Delete => {
                let Some(n) = self.current() else {
                    return DialogOutcome::Consumed;
                };
                if n < self.saved.len() {
                    self.saved.remove(n);
                }
                self.move_to(self.cursor);
                return DialogOutcome::Act(DialogResult::Text(format!("forget:{n}")));
            }
            KeyCode::Backspace => {
                self.search.pop();
                self.move_to(0);
            }
            _ => match key.text() {
                Some(c) => {
                    self.search.push(c);
                    // The first match, or the panels when nothing matches.
                    self.move_to(usize::from(self.shown().len() > 1));
                }
                None => return DialogOutcome::Ignored,
            },
        }
        DialogOutcome::Consumed
    }

    fn render(&self, f: &mut Frame, area: Rect, style: &DialogStyle) {
        let width = usize::from(area.width);
        let ascii = style.ascii;
        let body = style.body();
        let put = |f: &mut Frame, at: u16, text: String, st: ratatui::style::Style| {
            if let Some(rect) = row(area, at) {
                f.render_widget(
                    Paragraph::new(Line::from(Span::styled(format!("{text:<width$}"), st))),
                    rect,
                );
            }
        };
        let search = if self.search.is_empty() {
            " Type to search".to_string()
        } else {
            format!(" Search: {}", self.search)
        };
        put(f, 0, search, body);
        let arrow = if ascii { " <-> " } else { " \u{2194} " };
        let side_width = width.saturating_sub(4 + text::width(arrow)) / 2;
        let draw_choice = |f: &mut Frame, at: u16, choice: &Choice, selected: bool| {
            let (name_style, path_style) = if selected {
                (style.row_cursor(true), style.row_cursor(true))
            } else {
                (body.add_modifier(Modifier::BOLD), body)
            };
            put(f, at, format!(" {}", choice.name), name_style);
            let paths = format!(
                "   {}{arrow}{}",
                middle(&choice.left, side_width, ascii),
                middle(&choice.right, side_width, ascii)
            );
            put(f, at.saturating_add(1), paths, path_style);
        };
        // The panels, pinned: they are a different kind of choice from the
        // saved pairs below, and are never scrolled away or filtered out.
        draw_choice(f, 1, &self.here, self.cursor == 0);
        let shown = self.shown();
        let saved_count = shown.len().saturating_sub(1);
        let rule = if ascii { "-" } else { "\u{2500}" };
        let label = if self.search.is_empty() || saved_count > 0 {
            format!(" Saved ({saved_count}) ")
        } else {
            " Saved: none match ".to_string()
        };
        let fill = width.saturating_sub(text::width(&label) + 2);
        put(
            f,
            3,
            format!("{rule}{rule}{label}{}", rule.repeat(fill)),
            body,
        );
        for (offset, which) in shown
            .iter()
            .skip(1)
            .skip(self.top)
            .take(self.fits)
            .enumerate()
        {
            let Some(choice) = self.choice(*which) else {
                continue;
            };
            let at = u16::try_from(4 + offset * 2).unwrap_or(u16::MAX);
            draw_choice(f, at, choice, self.top + offset + 1 == self.cursor);
        }
    }
}

#[cfg(test)]
#[path = "syncpick_tests.rs"]
mod tests;
