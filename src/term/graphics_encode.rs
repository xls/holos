//! The three picture protocols as bytes, and the painter that sends them.
//!
//! Each encoder takes a picture already scaled to the pixels it will occupy
//! and returns the escape sequence that draws it at the cursor. The
//! [`Painter`] decides when to send one: only when what is on screen should
//! change, because a sixel of a full window is hundreds of kilobytes and the
//! event loop draws a frame on every key.

use std::io::Write;

use image::RgbaImage;

use super::graphics::Protocol;
use crate::viewer::image::over;

/// The id our kitty picture is placed under. One picture is ever shown, so one
/// id; reusing it replaces the previous picture.
const KITTY_ID: u32 = 4_242;

/// Bytes of base64 per kitty chunk; the protocol's limit.
const KITTY_CHUNK: usize = 4096;

/// A picture as the kitty graphics protocol: RGBA, zlib-compressed, sent in
/// chunks and placed at the cursor across `cols` x `rows` cells without moving
/// it. `q=2` asks for no reply, which would otherwise arrive as keystrokes.
#[must_use]
pub fn kitty(img: &RgbaImage, cols: u16, rows: u16) -> Vec<u8> {
    let raw = img.as_raw();
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    // Writing to a Vec cannot fail; the empty fallback sends an empty picture
    // rather than a panic path.
    let packed = z
        .write_all(raw)
        .and_then(|()| z.finish())
        .unwrap_or_default();
    let data = super::osc52::base64(&packed);
    let bytes = data.as_bytes();
    let mut out = Vec::with_capacity(bytes.len().saturating_add(256));
    let chunks: Vec<&[u8]> = bytes.chunks(KITTY_CHUNK).collect();
    let last = chunks.len().saturating_sub(1);
    if chunks.is_empty() {
        return out;
    }
    for (i, chunk) in chunks.iter().enumerate() {
        let more = u8::from(i != last);
        if i == 0 {
            let _ = write!(
                out,
                "\x1b_Ga=T,f=32,o=z,s={},v={},i={KITTY_ID},c={cols},r={rows},C=1,q=2,m={more};",
                img.width(),
                img.height()
            );
        } else {
            let _ = write!(out, "\x1b_Gm={more};");
        }
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\x1b\\");
    }
    out
}

/// Delete our kitty picture and its data.
#[must_use]
pub fn kitty_delete() -> Vec<u8> {
    format!("\x1b_Ga=d,d=I,i={KITTY_ID},q=2\x1b\\").into_bytes()
}

/// A picture as an iTerm2 inline image: a PNG, sized in cells.
#[must_use]
pub fn iterm2(img: &RgbaImage, cols: u16, rows: u16) -> Vec<u8> {
    let mut png = Vec::new();
    let encoded = image::ImageEncoder::write_image(
        image::codecs::png::PngEncoder::new(&mut png),
        img.as_raw(),
        img.width(),
        img.height(),
        image::ExtendedColorType::Rgba8,
    );
    if encoded.is_err() {
        return Vec::new();
    }
    format!(
        "\x1b]1337;File=inline=1;size={};width={cols};height={rows};preserveAspectRatio=0:{}\x07",
        png.len(),
        super::osc52::base64(&png)
    )
    .into_bytes()
}

/// The palette sixel pictures are drawn in: a 6 x 6 x 6 cube.
///
/// Fixed rather than chosen per picture, which costs some banding on a
/// photograph and buys an encoder with no search in it. The steps are the
/// usual `0, 51, ... 255`.
const LEVELS: u8 = 6;

/// The cube index of a colour.
fn cube_index(rgb: [u8; 3]) -> u8 {
    let step = |c: u8| -> u8 {
        let level = (u16::from(c) * u16::from(LEVELS - 1) + 127) / 255;
        u8::try_from(level).unwrap_or(LEVELS - 1)
    };
    let [r, g, b] = rgb;
    step(r)
        .saturating_mul(LEVELS * LEVELS)
        .saturating_add(step(g).saturating_mul(LEVELS))
        .saturating_add(step(b))
}

/// A cube index as sixel's percentages.
fn cube_percent(index: u8) -> (u16, u16, u16) {
    let level = |l: u8| -> u16 { u16::from(l) * 100 / u16::from(LEVELS - 1) };
    (
        level(index / (LEVELS * LEVELS)),
        level((index / LEVELS) % LEVELS),
        level(index % LEVELS),
    )
}

/// A picture as sixel, laid over `bg` where it is transparent.
#[must_use]
pub fn sixel(img: &RgbaImage, bg: [u8; 3]) -> Vec<u8> {
    let (width, height) = img.dimensions();
    let index: Vec<u8> = img.pixels().map(|p| cube_index(over(*p, bg))).collect();
    let at = |x: u32, y: u32| -> Option<u8> {
        let i = usize::try_from(u64::from(y) * u64::from(width) + u64::from(x)).ok()?;
        index.get(i).copied()
    };

    let mut out = Vec::new();
    let _ = write!(out, "\x1bPq\"1;1;{width};{height}");
    let mut used = [false; 256];
    for &i in &index {
        if let Some(slot) = used.get_mut(usize::from(i)) {
            *slot = true;
        }
    }
    for (i, _) in used.iter().enumerate().filter(|(_, u)| **u) {
        let Ok(i) = u8::try_from(i) else { continue };
        let (r, g, b) = cube_percent(i);
        let _ = write!(out, "#{i};2;{r};{g};{b}");
    }

    let mut band = 0_u32;
    while band < height {
        let mut colours = [false; 256];
        for y in band..band.saturating_add(6).min(height) {
            for x in 0..width {
                if let Some(c) = at(x, y)
                    && let Some(slot) = colours.get_mut(usize::from(c))
                {
                    *slot = true;
                }
            }
        }
        let mut first = true;
        for (colour, _) in colours.iter().enumerate().filter(|(_, u)| **u) {
            let Ok(colour) = u8::try_from(colour) else {
                continue;
            };
            if !first {
                out.push(b'$');
            }
            first = false;
            let _ = write!(out, "#{colour}");
            let row: Vec<u8> = (0..width)
                .map(|x| {
                    let mut bits = 0_u8;
                    for dy in 0..6 {
                        if at(x, band.saturating_add(dy)) == Some(colour) {
                            bits |= 1 << dy;
                        }
                    }
                    63_u8.saturating_add(bits)
                })
                .collect();
            run_length(&row, &mut out);
        }
        out.push(b'-');
        band = band.saturating_add(6);
    }
    out.extend_from_slice(b"\x1b\\");
    out
}

/// Sixel's run-length form: `!<n><char>` for a run longer than three.
fn run_length(row: &[u8], out: &mut Vec<u8>) {
    let mut i = 0;
    while let Some(&c) = row.get(i) {
        let run = row
            .get(i..)
            .map_or(1, |rest| rest.iter().take_while(|&&b| b == c).count());
        if run > 3 {
            let _ = write!(out, "!{run}");
            out.push(c);
        } else {
            for _ in 0..run {
                out.push(c);
            }
        }
        i = i.saturating_add(run.max(1));
    }
}

/// What a frame wants on screen: a picture at a place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    /// Which viewer's picture, so a different file at the same place is a
    /// change.
    pub viewer: u64,
    /// Top-left cell.
    pub x: u16,
    /// Top-left row.
    pub y: u16,
    /// Columns.
    pub cols: u16,
    /// Rows.
    pub rows: u16,
    /// The scaled picture's pixel size.
    pub px: (u32, u32),
    /// How it is drawn.
    pub protocol: Protocol,
}

/// What has been sent, so a frame that shows the same picture in the same
/// place sends nothing.
#[derive(Debug, Default)]
pub struct Painter {
    shown: Option<Placement>,
}

impl Painter {
    /// A painter that has sent nothing.
    #[must_use]
    pub const fn new() -> Self {
        Self { shown: None }
    }

    /// Before the frame: whether the screen must be cleared and redrawn whole.
    ///
    /// A sixel or iTerm2 picture is written into the cells, and the frame
    /// diff does not know it is there - it would leave the old picture behind
    /// wherever the new frame's text happens to match the last one's. A full
    /// redraw is what removes it.
    #[must_use]
    pub fn must_clear(&self, next: Option<&Placement>) -> bool {
        self.shown
            .as_ref()
            .is_some_and(|was| was.protocol.writes_cells() && Some(was) != next)
    }

    /// After the frame: send what changed. `picture` is the scaled picture
    /// for `next`.
    ///
    /// # Errors
    ///
    /// A write to the terminal failed.
    pub fn paint<W: Write>(
        &mut self,
        out: &mut W,
        next: Option<Placement>,
        picture: Option<&RgbaImage>,
        bg: [u8; 3],
    ) -> std::io::Result<()> {
        if self.shown == next {
            return Ok(());
        }
        if self
            .shown
            .is_some_and(|was| was.protocol == Protocol::Kitty)
        {
            out.write_all(&kitty_delete())?;
        }
        self.shown = None;
        if let (Some(place), Some(img)) = (next, picture) {
            let bytes = match place.protocol {
                Protocol::Kitty => kitty(img, place.cols, place.rows),
                Protocol::Sixel => sixel(img, bg),
                Protocol::Iterm2 => iterm2(img, place.cols, place.rows),
                Protocol::Halfblocks | Protocol::Blocks | Protocol::Off => Vec::new(),
            };
            if !bytes.is_empty() {
                // Save the cursor, go to the cell, draw, come back: the frame
                // has put the cursor where it wants it.
                write!(
                    out,
                    "\x1b7\x1b[{};{}H",
                    place.y.saturating_add(1),
                    place.x.saturating_add(1)
                )?;
                out.write_all(&bytes)?;
                out.write_all(b"\x1b8")?;
                self.shown = Some(place);
            }
        }
        out.flush()
    }
}

#[cfg(test)]
#[path = "graphics_encode_tests.rs"]
mod tests;
