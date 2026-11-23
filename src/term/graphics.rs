//! Which way this terminal can draw a picture, and how we find out.
//!
//! Three protocols put real pixels in a terminal, and none of them is
//! universal:
//!
//! * **kitty graphics** - kitty, Ghostty, WezTerm. The picture is a layer over
//!   the cells, placed and deleted by id.
//! * **sixel** - foot, xterm, mlterm, WezTerm, Windows Terminal. The picture is
//!   written into the cells and stays until text is written over it.
//! * **iTerm2 inline images** - iTerm2, WezTerm. Like sixel in that it is
//!   written into the cells, but it carries an encoded file.
//!
//! Everything else gets half blocks: `▀` in the top pixel's colour on the
//! bottom pixel's, two pixels a cell, which any colour terminal can show.
//!
//! # Finding out
//!
//! The environment first, because it costs nothing: `TERM=xterm-kitty`,
//! `TERM_PROGRAM=ghostty`, `TERM=foot`. It is a hint only - it is wrong inside
//! tmux and frequently missing over ssh - so where it says nothing the
//! terminal is asked: a kitty graphics query that a kitty-capable terminal
//! answers, followed by Device Attributes, which every terminal answers and
//! whose list says `4` where sixel is supported. The second reply ending is
//! how we know the first is not coming, so there is no guessing at a timeout
//! for a terminal that does reply.
//!
//! The query runs at start-up, before the keyboard query and before
//! crossterm's reader exists, so nothing else is reading the replies. A
//! terminal that answers neither within the keyboard query's bound is one
//! whose replies cannot be read at all, and the keyboard query is then
//! skipped too rather than waited out a second time.

use std::io::Write;
use std::time::{Duration, Instant};

use crate::config::GraphicsSetting;

/// Overrides `terminal.graphics`, the way `HCMD_KEYBOARD_PROTOCOL` overrides
/// the keyboard: a test harness has a pty and no config.
pub const GRAPHICS_ENV: &str = "HCMD_GRAPHICS";

/// How pictures are drawn this session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Protocol {
    /// The kitty graphics protocol.
    Kitty,
    /// DEC sixel.
    Sixel,
    /// iTerm2 inline images.
    Iterm2,
    /// Coloured `▀` cells.
    #[default]
    Halfblocks,
    /// No pictures.
    Off,
}

impl Protocol {
    /// Whether the picture is written into the cells, where text written later
    /// replaces it. True for sixel and iTerm2; kitty keeps its pictures on a
    /// layer of their own, and half blocks *are* text.
    #[must_use]
    pub const fn writes_cells(self) -> bool {
        matches!(self, Self::Sixel | Self::Iterm2)
    }

    /// Whether the picture is drawn by escape sequences after the frame,
    /// rather than as characters inside it.
    #[must_use]
    pub const fn is_pixels(self) -> bool {
        matches!(self, Self::Kitty | Self::Sixel | Self::Iterm2)
    }

    /// The name the setting uses.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Kitty => "kitty",
            Self::Sixel => "sixel",
            Self::Iterm2 => "iterm2",
            Self::Halfblocks => "halfblocks",
            Self::Off => "off",
        }
    }
}

/// What the terminal said about itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Answer {
    /// It answered the kitty graphics query with `OK`.
    pub kitty: bool,
    /// Its Device Attributes list sixel (`4`).
    pub sixel: bool,
}

/// The setting, read from the environment override first.
#[must_use]
pub fn setting(configured: GraphicsSetting) -> GraphicsSetting {
    std::env::var(GRAPHICS_ENV)
        .ok()
        .and_then(|raw| parse_setting(&raw))
        .unwrap_or(configured)
}

/// `HCMD_GRAPHICS`'s value as a setting, or `None` for one we do not know.
#[must_use]
pub fn parse_setting(raw: &str) -> Option<GraphicsSetting> {
    Some(match raw.trim().to_ascii_lowercase().as_str() {
        "auto" => GraphicsSetting::Auto,
        "kitty" => GraphicsSetting::Kitty,
        "sixel" => GraphicsSetting::Sixel,
        "iterm2" => GraphicsSetting::Iterm2,
        "halfblocks" => GraphicsSetting::Halfblocks,
        "off" => GraphicsSetting::Off,
        _ => return None,
    })
}

/// What the environment says, or `None` when it says nothing useful.
///
/// `env` is the lookup, so a test can hand it a table.
pub fn from_env<F: Fn(&str) -> Option<String>>(env: F) -> Option<Protocol> {
    let has = |key: &str| env(key).is_some_and(|v| !v.is_empty());
    let term = env("TERM").unwrap_or_default().to_ascii_lowercase();
    let program = env("TERM_PROGRAM").unwrap_or_default().to_ascii_lowercase();
    // A multiplexer sits between us and the terminal, rewrites `TERM`, and
    // passes none of the three through without wrapping. Half blocks are what
    // it can carry.
    if has("TMUX") || term.starts_with("screen") || term.starts_with("tmux") {
        return Some(Protocol::Halfblocks);
    }
    if term == "xterm-kitty" || has("KITTY_WINDOW_ID") {
        return Some(Protocol::Kitty);
    }
    if term == "xterm-ghostty" || program == "ghostty" || has("GHOSTTY_RESOURCES_DIR") {
        return Some(Protocol::Kitty);
    }
    if program == "wezterm" {
        return Some(Protocol::Kitty);
    }
    if program == "iterm.app" {
        return Some(Protocol::Iterm2);
    }
    if term.starts_with("foot") || term.starts_with("mlterm") {
        return Some(Protocol::Sixel);
    }
    None
}

/// The protocol for a setting, an environment guess and a query's answer.
#[must_use]
pub const fn resolve(
    setting: GraphicsSetting,
    guess: Option<Protocol>,
    answer: Option<Answer>,
) -> Protocol {
    match setting {
        GraphicsSetting::Kitty => Protocol::Kitty,
        GraphicsSetting::Sixel => Protocol::Sixel,
        GraphicsSetting::Iterm2 => Protocol::Iterm2,
        GraphicsSetting::Halfblocks => Protocol::Halfblocks,
        GraphicsSetting::Off => Protocol::Off,
        GraphicsSetting::Auto => match (guess, answer) {
            (Some(protocol), _) => protocol,
            (None, Some(Answer { kitty: true, .. })) => Protocol::Kitty,
            (None, Some(Answer { sixel: true, .. })) => Protocol::Sixel,
            (None, _) => Protocol::Halfblocks,
        },
    }
}

/// The query: a one-pixel kitty graphics query, then Device Attributes.
pub const QUERY: &[u8] = b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c";

/// Read the replies to [`QUERY`] out of what arrived. `None` until the Device
/// Attributes reply is complete, which is the end of the answer.
#[must_use]
pub fn parse_replies(bytes: &[u8]) -> Option<Answer> {
    let kitty = find(bytes, b"\x1b_Gi=31;OK").is_some();
    // `ESC [ ? <params> c`, where the params are digits and semicolons.
    let mut from = 0;
    while let Some(start) = find(bytes.get(from..)?, b"\x1b[?") {
        let params_at = from.saturating_add(start).saturating_add(3);
        let rest = bytes.get(params_at..)?;
        let len = rest
            .iter()
            .take_while(|b| b.is_ascii_digit() || **b == b';')
            .count();
        match rest.get(len) {
            Some(b'c') => {
                let params = rest.get(..len)?;
                let sixel = params.split(|b| *b == b';').any(|p| p == b"4");
                return Some(Answer { kitty, sixel });
            }
            // Still arriving.
            None => return None,
            // Some other `CSI ?` reply; keep looking past it.
            Some(_) => from = params_at,
        }
    }
    None
}

/// Where `needle` first occurs in `hay`.
fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Ask the terminal, waiting at most `timeout` for the Device Attributes reply.
///
/// `None` when it did not come: the terminal answers nothing we can read.
/// Raw mode must already be on. Reading uses the terminal's own read timeout
/// (`VMIN` 0, `VTIME` a tenth of a second) rather than `poll`, which macOS does
/// not implement for terminals; the settings are put back before returning.
#[must_use]
pub fn query(timeout: Duration) -> Option<Answer> {
    use rustix::termios::{OptionalActions, SpecialCodeIndex, tcgetattr, tcsetattr};
    use std::os::fd::AsFd;

    let stdin = std::io::stdin();
    let fd = stdin.as_fd();
    let saved = tcgetattr(fd).ok()?;
    let mut timed = saved.clone();
    timed.special_codes[SpecialCodeIndex::VMIN] = 0;
    timed.special_codes[SpecialCodeIndex::VTIME] = 1;
    tcsetattr(fd, OptionalActions::Now, &timed).ok()?;

    let answer = ask(fd, timeout);

    // Whatever happened above, the terminal's read settings go back.
    let _ = tcsetattr(fd, OptionalActions::Now, &saved);
    answer
}

/// Send [`QUERY`] and read until [`parse_replies`] is satisfied or time is up.
fn ask(fd: std::os::fd::BorrowedFd<'_>, timeout: Duration) -> Option<Answer> {
    let mut out = std::io::stdout();
    out.write_all(QUERY).ok()?;
    out.flush().ok()?;
    let deadline = Instant::now() + timeout;
    let mut got: Vec<u8> = Vec::new();
    let mut buf = [0_u8; 256];
    while Instant::now() < deadline {
        // Returns 0 after a tenth of a second of nothing: `VTIME`.
        let n = rustix::io::read(fd, &mut buf).ok()?;
        got.extend_from_slice(buf.get(..n)?);
        if let Some(answer) = parse_replies(&got) {
            return Some(answer);
        }
        // A terminal that answers with a flood is not answering this.
        if got.len() > 4096 {
            return None;
        }
    }
    None
}

/// The size of one cell in pixels, from the terminal's window size.
///
/// `(8, 16)` - the proportion every monospace font is near - when the terminal
/// does not report pixels, which some do not.
#[must_use]
pub fn cell_pixels() -> (u16, u16) {
    crossterm::terminal::window_size()
        .ok()
        .and_then(|size| {
            let w = size.width.checked_div(size.columns)?;
            let h = size.height.checked_div(size.rows)?;
            (w > 0 && h > 0).then_some((w, h))
        })
        .unwrap_or(FALLBACK_CELL)
}

/// The cell size assumed when the terminal does not say.
pub const FALLBACK_CELL: (u16, u16) = (8, 16);

#[cfg(test)]
#[path = "graphics_tests.rs"]
mod tests;
