//! Mode 3 for a picture: the image itself.
//!
//! Every other mode streams. A picture cannot - its last row is at the end of
//! the file and most formats compress across rows - so this reads the file
//! whole, which is why it has its own ceiling (`viewer.image_max_size`) and
//! why the read and the decode happen on the blocking pool and never in
//! `dispatch` or on the drawing thread. The viewer asks for the decode by
//! handing out an [`ImageJob`] once, the event loop runs [`decode`] and sends
//! the [`ImageBatch`] back, exactly the shape of the line index.
//!
//! What is kept is bounded by the screen, not the file: the decoded picture is
//! scaled down to [`MAX_SIDE`] at once, and what is drawn is a second copy
//! scaled to the cells it occupies, made again only when that size changes.
//!
//! The summary document a template makes of the file is still built as
//! before; it is what the `i` box shows beside the picture, and what mode 3
//! falls back to when there is no picture to show.

use std::sync::Arc;

use image::RgbaImage;

use super::source::{Opener, Source, WindowLen};
use super::{Viewer, ViewerId};
use crate::config::ViewerMode;

/// The longest side a decoded picture is kept at.
///
/// Bigger than any terminal is wide in pixels, so nothing visible is lost, and
/// small enough that the copy held per viewer is a few megabytes whatever the
/// file said its dimensions were.
pub const MAX_SIDE: u32 = 2048;

/// The most a picture is enlarged to fill the screen. A 16 pixel icon at
/// eight times is something to look at; at sixty it is a blur of squares.
const MAX_ZOOM: f64 = 8.0;

/// The largest width or height a file may claim. A header is a claim, not a
/// measurement, and the decoder allocates from it.
const MAX_CLAIMED_SIDE: u32 = 32_768;

/// The most the decoder may allocate for one picture.
const MAX_ALLOC: u64 = 1 << 30;

/// The label of the one-line summary a picture no template describes gets.
pub const FALLBACK_LABEL: &str = "image";

/// The extensions mode 3 shows as a picture: the formats the `image` crate is
/// built with here.
const EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "jpe", "gif", "bmp", "dib", "ico", "tga", "tif", "tiff", "webp",
];

/// Whether a name is one mode 3 shows as a picture.
///
/// By extension, as [`super::render::RenderKind::of_name`] is and for the same
/// reason: sniffing would read the file to decide whether to read the file. A
/// `.png` that is not one fails to decode and falls back to its summary.
#[must_use]
pub fn is_image_name(name: &str) -> bool {
    let Some((_, ext)) = name.rsplit_once('.') else {
        return false;
    };
    let ext = ext.to_ascii_lowercase();
    EXTENSIONS.contains(&ext.as_str())
}

/// A decoded picture and what the decoder said about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    /// The pixels, scaled down to [`MAX_SIDE`] on the long side.
    pub pixels: RgbaImage,
    /// The width the file has.
    pub width: u32,
    /// The height the file has.
    pub height: u32,
    /// The format, as the decoder recognised it: `PNG`, `JPEG`.
    pub format: String,
    /// The colour type the file stores: `RGBA, 8 bits per channel`.
    pub color: String,
    /// How many bytes were read.
    pub bytes: u64,
}

/// Where a viewer's picture is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageState {
    /// The event loop has the job; nothing has come back yet.
    Pending,
    /// Decoded.
    Ready(Arc<Picture>),
    /// Not a picture after all, or too big, or unreadable: mode 3 shows the
    /// summary and the status line says this.
    Failed(String),
}

/// The decode a viewer is owed.
pub struct ImageJob {
    /// Which viewer the answer belongs to.
    pub id: ViewerId,
    /// A fresh cursor over the same file, as the index task gets.
    pub opener: Opener,
    /// The size the backend reported.
    pub len: Option<u64>,
    /// `viewer.image_max_size`.
    pub limit: u64,
}

impl std::fmt::Debug for ImageJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageJob")
            .field("id", &self.id)
            .field("len", &self.len)
            .field("limit", &self.limit)
            .finish_non_exhaustive()
    }
}

/// What came back from [`decode`].
#[derive(Debug, Clone)]
pub struct ImageBatch {
    /// Which viewer it is for.
    pub id: ViewerId,
    /// The picture, or why there is none.
    pub result: Result<Arc<Picture>, String>,
}

/// Read and decode a picture. Runs on the blocking pool.
#[must_use]
pub fn decode(job: ImageJob) -> ImageBatch {
    let id = job.id;
    let result = read_whole(&job).and_then(|bytes| decode_bytes(&bytes));
    ImageBatch {
        id,
        result: result.map(Arc::new),
    }
}

/// The file, whole, or a refusal naming the ceiling.
fn read_whole(job: &ImageJob) -> Result<Vec<u8>, String> {
    if let Some(len) = job.len
        && len > job.limit
    {
        return Err(too_big(len, job.limit));
    }
    let mut source = Source::open(Arc::clone(&job.opener), job.len).map_err(|e| e.to_string())?;
    let mut out: Vec<u8> = Vec::new();
    let mut at = 0_u64;
    loop {
        let window = source
            .read_window(at, WindowLen::MAX)
            .map_err(|e| e.to_string())?;
        let bytes = window.bytes();
        if bytes.is_empty() {
            return Ok(out);
        }
        out.extend_from_slice(bytes);
        at = at.saturating_add(bytes.len() as u64);
        // A size nobody could `stat` is learned by reading, so the ceiling is
        // checked as the bytes arrive and not only against the claim above.
        if at > job.limit {
            return Err(too_big(at, job.limit));
        }
    }
}

fn too_big(len: u64, limit: u64) -> String {
    format!(
        "image is {}, over viewer.image_max_size ({})",
        crate::panel::format::human_size(len),
        crate::panel::format::human_size(limit)
    )
}

/// Decode bytes into a [`Picture`].
///
/// # Errors
///
/// The decoder's own message: an unknown format, a corrupt file, or a header
/// claiming more than [`MAX_CLAIMED_SIDE`] or [`MAX_ALLOC`] allows.
pub fn decode_bytes(bytes: &[u8]) -> Result<Picture, String> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let format = reader
        .format()
        .ok_or_else(|| "not a picture this viewer can decode".to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_CLAIMED_SIDE);
    limits.max_image_height = Some(MAX_CLAIMED_SIDE);
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    let decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    let color = describe_color(image::ImageDecoder::original_color_type(&decoder));
    let decoded = image::DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    let (width, height) = (decoded.width(), decoded.height());
    let (w, h) = shrink_to(width, height, MAX_SIDE);
    let pixels = if (w, h) == (width, height) {
        decoded.into_rgba8()
    } else {
        decoded.thumbnail_exact(w, h).into_rgba8()
    };
    Ok(Picture {
        pixels,
        width,
        height,
        format: format_name(format).to_string(),
        color,
        bytes: bytes.len() as u64,
    })
}

/// The format's usual name.
const fn format_name(format: image::ImageFormat) -> &'static str {
    match format {
        image::ImageFormat::Png => "PNG",
        image::ImageFormat::Jpeg => "JPEG",
        image::ImageFormat::Gif => "GIF",
        image::ImageFormat::WebP => "WebP",
        image::ImageFormat::Bmp => "BMP",
        image::ImageFormat::Ico => "ICO",
        image::ImageFormat::Tiff => "TIFF",
        image::ImageFormat::Tga => "TGA",
        // `ImageFormat` is the crate's and non-exhaustive; the ones above are
        // the features this binary is built with.
        _ => "image",
    }
}

/// How a colour type reads to a person.
fn describe_color(color: image::ExtendedColorType) -> String {
    use image::ExtendedColorType as C;
    let (what, bits) = match color {
        C::L1 => ("grey", 1),
        C::La1 => ("grey + alpha", 1),
        C::L2 => ("grey", 2),
        C::La2 => ("grey + alpha", 2),
        C::L4 => ("grey", 4),
        C::La4 => ("grey + alpha", 4),
        C::L8 => ("grey", 8),
        C::La8 => ("grey + alpha", 8),
        C::L16 => ("grey", 16),
        C::La16 => ("grey + alpha", 16),
        C::Rgb1 => ("RGB", 1),
        C::Rgb2 => ("RGB", 2),
        C::Rgb4 => ("RGB", 4),
        C::Rgb8 | C::Bgr8 => ("RGB", 8),
        C::Rgba1 => ("RGBA", 1),
        C::Rgba2 => ("RGBA", 2),
        C::Rgba4 => ("RGBA", 4),
        C::Rgba8 | C::Bgra8 => ("RGBA", 8),
        C::Rgb16 => ("RGB", 16),
        C::Rgba16 => ("RGBA", 16),
        C::Rgb32F => ("RGB float", 32),
        C::Rgba32F => ("RGBA float", 32),
        C::Cmyk8 => ("CMYK", 8),
        C::Cmyk16 => ("CMYK", 16),
        C::Unknown(bits) => return format!("{bits} bits per pixel"),
        // Non-exhaustive and the crate's.
        other => return format!("{other:?}"),
    };
    format!("{what}, {bits} bits per channel")
}

/// `width` x `height` scaled so neither side exceeds `side`, keeping the
/// proportion. Never smaller than one pixel either way.
#[must_use]
pub fn shrink_to(width: u32, height: u32, side: u32) -> (u32, u32) {
    let long = width.max(height);
    if long <= side || long == 0 {
        return (width.max(1), height.max(1));
    }
    let scale = f64::from(side) / f64::from(long);
    (scaled(width, scale), scaled(height, scale))
}

/// One side times a scale, rounded, at least one.
fn scaled(side: u32, scale: f64) -> u32 {
    let px = (f64::from(side) * scale).round();
    // Bounded by the caller: the scale never takes a side past `u32` range.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "rounded, non-negative, and bounded by the area it is fitted to"
    )]
    let px = px as u32;
    px.max(1)
}

/// Where a picture goes inside an area of cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fit {
    /// Column offset of the picture inside the area.
    pub x: u16,
    /// Row offset of the picture inside the area.
    pub y: u16,
    /// Columns it occupies.
    pub cols: u16,
    /// Rows it occupies.
    pub rows: u16,
    /// Its size in pixels: what is drawn into those cells.
    pub px_width: u32,
    /// Its height in pixels.
    pub px_height: u32,
    /// How much of the file's own size this is, in percent.
    pub percent: u32,
}

/// Fit a `width` x `height` picture into `cols` x `rows` cells of
/// `cell_w` x `cell_h` pixels each: as large as fits, proportion kept,
/// enlarged no more than [`MAX_ZOOM`] times, centred.
///
/// `None` when there is no room at all.
#[must_use]
pub fn fit(width: u32, height: u32, cols: u16, rows: u16, cell: (u16, u16)) -> Option<Fit> {
    let (cell_w, cell_h) = (u32::from(cell.0.max(1)), u32::from(cell.1.max(1)));
    let area_w = u32::from(cols).saturating_mul(cell_w);
    let area_h = u32::from(rows).saturating_mul(cell_h);
    if width == 0 || height == 0 || area_w == 0 || area_h == 0 {
        return None;
    }
    let scale = (f64::from(area_w) / f64::from(width))
        .min(f64::from(area_h) / f64::from(height))
        .min(MAX_ZOOM);
    let px_width = scaled(width, scale).min(area_w);
    let px_height = scaled(height, scale).min(area_h);
    let used_cols = u16::try_from(px_width.div_ceil(cell_w))
        .unwrap_or(cols)
        .min(cols);
    let used_rows = u16::try_from(px_height.div_ceil(cell_h))
        .unwrap_or(rows)
        .min(rows);
    let percent = (scale * 100.0).round();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a percentage between 0 and MAX_ZOOM * 100"
    )]
    let percent = percent as u32;
    Some(Fit {
        x: (cols.saturating_sub(used_cols)) / 2,
        y: (rows.saturating_sub(used_rows)) / 2,
        cols: used_cols,
        rows: used_rows,
        px_width,
        px_height,
        percent,
    })
}

/// A pixel laid over a background, alpha applied.
#[must_use]
pub fn over(pixel: image::Rgba<u8>, bg: [u8; 3]) -> [u8; 3] {
    let [r, g, b, a] = pixel.0;
    let mix = |fg: u8, back: u8| -> u8 {
        let value = (u16::from(fg) * u16::from(a) + u16::from(back) * (255 - u16::from(a))) / 255;
        u8::try_from(value).unwrap_or(u8::MAX)
    };
    let [br, bgg, bb] = bg;
    [mix(r, br), mix(g, bgg), mix(b, bb)]
}

/// The colours of the cell at `(col, row)` when the picture is drawn two
/// pixels to a cell, top and bottom: what a `▀` in the top colour on the
/// bottom colour shows.
///
/// `scaled` is the picture already at one pixel per column and two per row.
#[must_use]
pub fn half_block(scaled: &RgbaImage, col: u32, row: u32, bg: [u8; 3]) -> ([u8; 3], [u8; 3]) {
    let at = |y: u32| {
        scaled
            .get_pixel_checked(col, y)
            .map_or(bg, |pixel| over(*pixel, bg))
    };
    let top = row.saturating_mul(2);
    (at(top), at(top.saturating_add(1)))
}

impl Viewer {
    /// Whether mode 3 shows this file as a picture.
    #[must_use]
    pub fn is_picture(&self) -> bool {
        self.mode == ViewerMode::Render && is_image_name(&self.format_name())
    }

    /// The decode this viewer is owed, handed out once.
    ///
    /// Only in mode 3 and only for a picture: a `.png` read in hex never pays
    /// for a decode. Taking it marks the picture pending, so a frame drawn
    /// before the answer says "decoding" rather than showing the summary and
    /// then jumping.
    pub fn take_image_job(&mut self) -> Option<ImageJob> {
        if !self.is_picture() || self.image.is_some() {
            return None;
        }
        self.image = Some(ImageState::Pending);
        Some(ImageJob {
            id: self.id,
            opener: self.source.opener(),
            len: self.source.len(),
            limit: self.image_max,
        })
    }

    /// Apply a decode. False when it belongs to another viewer.
    pub fn apply_image(&mut self, batch: &ImageBatch) -> bool {
        if batch.id != self.id {
            return false;
        }
        self.image_scaled = None;
        self.image = Some(match &batch.result {
            Ok(picture) => ImageState::Ready(Arc::clone(picture)),
            Err(why) => {
                self.render_note = Some(format!("{}: {why}", self.title));
                ImageState::Failed(why.clone())
            }
        });
        true
    }

    /// Where this viewer's picture is, if it has been asked for.
    #[must_use]
    pub const fn image_state(&self) -> Option<&ImageState> {
        self.image.as_ref()
    }

    /// The decoded picture, when there is one and mode 3 is showing it.
    #[must_use]
    pub fn picture(&self) -> Option<&Arc<Picture>> {
        match (&self.image, self.is_picture()) {
            (Some(ImageState::Ready(picture)), true) => Some(picture),
            _ => None,
        }
    }

    /// Make the copy of the picture that is drawn: `width` x `height` pixels.
    ///
    /// Called by the event loop before the draw, with the size the layout
    /// gives; a no-op while that size has not changed, so a picture is
    /// resampled when the window is resized and not once a frame.
    pub fn prepare_scaled(&mut self, width: u32, height: u32) {
        let Some(picture) = self.picture().cloned() else {
            return;
        };
        if self
            .image_scaled
            .as_ref()
            .is_some_and(|img| img.dimensions() == (width, height))
        {
            return;
        }
        let filter = if width >= picture.pixels.width() {
            // Enlarging an icon: the pixels as squares, not a smear.
            image::imageops::FilterType::Nearest
        } else {
            image::imageops::FilterType::Triangle
        };
        self.image_scaled = Some(image::imageops::resize(
            &picture.pixels,
            width.max(1),
            height.max(1),
            filter,
        ));
    }

    /// The copy [`Viewer::prepare_scaled`] made.
    #[must_use]
    pub const fn scaled_picture(&self) -> Option<&RgbaImage> {
        self.image_scaled.as_ref()
    }

    /// Forget the picture so the next frame decodes it again: `F2`.
    pub(crate) fn forget_image(&mut self) {
        self.image = None;
        self.image_scaled = None;
    }
}

#[cfg(test)]
#[path = "image_tests.rs"]
mod tests;
