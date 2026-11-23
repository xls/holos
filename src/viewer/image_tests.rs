use super::*;
use crate::config::ViewerConfig;
use crate::viewer::source;

/// A real PNG: `w` x `h`, the left half red and the right half half-transparent blue.
pub(crate) fn png(w: u32, h: u32) -> Vec<u8> {
    let img = RgbaImage::from_fn(w, h, |x, _| {
        if x < w / 2 {
            image::Rgba([255, 0, 0, 255])
        } else {
            image::Rgba([0, 0, 255, 128])
        }
    });
    let mut out = Vec::new();
    image::ImageEncoder::write_image(
        image::codecs::png::PngEncoder::new(&mut out),
        img.as_raw(),
        w,
        h,
        image::ExtendedColorType::Rgba8,
    )
    .unwrap();
    out
}

fn viewer(name: &str, body: &[u8], cfg: &ViewerConfig) -> Viewer {
    let bytes = Arc::new(body.to_vec());
    let len = bytes.len() as u64;
    Viewer::open(
        ViewerId(7),
        name,
        None,
        source::memory_opener(bytes),
        Some(len),
        cfg,
    )
    .unwrap()
}

fn job(body: &[u8], limit: u64) -> ImageJob {
    let bytes = Arc::new(body.to_vec());
    ImageJob {
        id: ViewerId(7),
        len: Some(bytes.len() as u64),
        opener: source::memory_opener(bytes),
        limit,
    }
}

#[test]
fn pictures_are_known_by_extension_in_any_case() {
    for name in [
        "a.png", "B.JPG", "c.jpeg", "d.gif", "e.webp", "f.tiff", "g.ico", "h.tga", "i.bmp",
    ] {
        assert!(is_image_name(name), "{name}");
    }
    for name in ["png", "a.svg", "a.png.txt", "notes", "a.pdf"] {
        assert!(!is_image_name(name), "{name}");
    }
}

#[test]
fn a_png_decodes_with_its_size_format_and_colour() {
    let picture = decode_bytes(&png(40, 20)).unwrap();
    assert_eq!((picture.width, picture.height), (40, 20));
    assert_eq!(picture.pixels.dimensions(), (40, 20));
    assert_eq!(picture.format, "PNG");
    assert_eq!(picture.color, "RGBA, 8 bits per channel");
}

#[test]
fn a_big_picture_is_kept_at_max_side_but_reports_its_own_size() {
    let picture = decode_bytes(&png(MAX_SIDE * 2, 10)).unwrap();
    assert_eq!((picture.width, picture.height), (MAX_SIDE * 2, 10));
    assert_eq!(picture.pixels.dimensions(), (MAX_SIDE, 5));
}

#[test]
fn garbage_is_refused_with_a_reason_not_a_panic() {
    let err = decode_bytes(b"this is not a picture at all").unwrap_err();
    assert!(!err.is_empty());
    // A PNG cut off after its header claims pixels it does not have.
    let mut cut = png(64, 64);
    cut.truncate(60);
    assert!(decode_bytes(&cut).is_err());
}

#[test]
fn a_header_claiming_an_enormous_picture_is_refused_before_allocating() {
    // A valid IHDR for 100000 x 100000 RGBA - 40 GB if believed.
    let mut ihdr = b"IHDR".to_vec();
    ihdr.extend_from_slice(&100_000_u32.to_be_bytes());
    ihdr.extend_from_slice(&100_000_u32.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    let crc = crc32fast::hash(&ihdr);
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    png.extend_from_slice(&13_u32.to_be_bytes());
    png.extend_from_slice(&ihdr);
    png.extend_from_slice(&crc.to_be_bytes());
    let err = decode_bytes(&png).unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn the_decode_job_refuses_a_file_over_the_ceiling_and_names_the_setting() {
    let body = png(32, 32);
    let batch = decode(job(&body, 10));
    assert_eq!(batch.id, ViewerId(7));
    let err = batch.result.unwrap_err();
    assert!(err.contains("viewer.image_max_size"), "{err}");

    let batch = decode(job(&body, 1 << 20));
    assert_eq!(batch.result.unwrap().width, 32);
}

#[test]
fn the_ceiling_holds_for_a_file_whose_size_was_not_reported() {
    let body = png(64, 64);
    let mut no_len = job(&body, 16);
    no_len.len = None;
    let err = decode(no_len).result.unwrap_err();
    assert!(err.contains("viewer.image_max_size"), "{err}");
}

#[test]
fn fit_keeps_the_proportion_centres_and_reports_the_zoom() {
    // 200 x 100 into 50 x 50 cells of 1 x 2 pixels: 50 x 100 pixels of room,
    // width-bound at a quarter.
    let f = fit(200, 100, 50, 50, (1, 2)).unwrap();
    assert_eq!((f.px_width, f.px_height), (50, 25));
    assert_eq!((f.cols, f.rows), (50, 13));
    assert_eq!((f.x, f.y), (0, 18));
    assert_eq!(f.percent, 25);
}

#[test]
fn fit_enlarges_an_icon_but_no_more_than_eight_times() {
    let f = fit(16, 16, 200, 100, (1, 2)).unwrap();
    assert_eq!((f.px_width, f.px_height), (128, 128));
    assert_eq!((f.cols, f.rows), (128, 64));
    assert_eq!(f.percent, 800);
}

#[test]
fn fit_with_no_room_is_none_rather_than_a_zero_sized_picture() {
    assert!(fit(10, 10, 0, 5, (1, 2)).is_none());
    assert!(fit(10, 10, 5, 0, (1, 2)).is_none());
    assert!(fit(0, 10, 5, 5, (1, 2)).is_none());
    // A cell size of zero is read as one, never divided by.
    assert!(fit(10, 10, 5, 5, (0, 0)).is_some());
}

#[test]
fn a_transparent_pixel_shows_the_background() {
    let bg = [10, 20, 30];
    assert_eq!(over(image::Rgba([200, 200, 200, 0]), bg), bg);
    assert_eq!(over(image::Rgba([200, 100, 0, 255]), bg), [200, 100, 0]);
    assert_eq!(
        over(image::Rgba([255, 255, 255, 128]), [0, 0, 0]),
        [128, 128, 128]
    );
}

#[test]
fn a_half_block_cell_is_two_pixels_top_and_bottom() {
    let img = RgbaImage::from_fn(2, 3, |x, y| {
        image::Rgba([x as u8 * 100, y as u8 * 50, 0, 255])
    });
    let bg = [9, 9, 9];
    assert_eq!(half_block(&img, 1, 0, bg), ([100, 0, 0], [100, 50, 0]));
    // The last row of an odd-height picture has no bottom pixel.
    assert_eq!(half_block(&img, 0, 1, bg), ([0, 100, 0], bg));
}

#[test]
fn a_png_in_mode_three_asks_for_one_decode_and_shows_the_picture() {
    let mut v = viewer("shot.png", &png(40, 20), &ViewerConfig::default());
    v.set_mode(ViewerMode::Render).unwrap();
    assert!(v.is_picture());
    let job = v.take_image_job().expect("a decode is owed");
    assert_eq!(v.image_state(), Some(&ImageState::Pending));
    assert!(v.take_image_job().is_none(), "and owed once");
    assert!(v.picture().is_none(), "nothing to show until it is back");

    let batch = decode(job);
    assert!(
        !v.apply_image(&ImageBatch {
            id: ViewerId(99),
            ..batch.clone()
        }),
        "not ours"
    );
    assert!(v.apply_image(&batch));
    assert_eq!(v.picture().unwrap().width, 40);

    v.prepare_scaled(20, 10);
    assert_eq!(v.scaled_picture().unwrap().dimensions(), (20, 10));
}

#[test]
fn a_png_read_as_hex_is_never_decoded() {
    let mut v = viewer("shot.png", &png(8, 8), &ViewerConfig::default());
    v.set_mode(ViewerMode::Hex).unwrap();
    assert!(!v.is_picture());
    assert!(v.take_image_job().is_none());
}

#[test]
fn a_failed_decode_leaves_the_summary_and_says_why() {
    let mut v = viewer(
        "broken.png",
        b"\x89PNG\r\n\x1a\nnot really",
        &ViewerConfig::default(),
    );
    v.set_mode(ViewerMode::Render).unwrap();
    let job = v.take_image_job().unwrap();
    assert!(v.apply_image(&decode(job)));
    assert!(matches!(v.image_state(), Some(ImageState::Failed(_))));
    assert!(v.picture().is_none());
    assert!(v.render_note().is_some_and(|n| n.contains("broken.png")));
}

#[test]
fn a_picture_no_template_knows_still_opens_in_mode_three() {
    // TGA has no magic number for a template to match.
    let mut v = viewer("sprite.tga", &[0_u8; 64], &ViewerConfig::default());
    v.set_mode(ViewerMode::Render).unwrap();
    assert_eq!(v.mode(), ViewerMode::Render);
    assert_eq!(v.rendered().map(|r| r.label.as_str()), Some(FALLBACK_LABEL));
}

#[test]
fn reloading_decodes_the_picture_again() {
    let mut v = viewer("shot.png", &png(8, 8), &ViewerConfig::default());
    v.set_mode(ViewerMode::Render).unwrap();
    let job = v.take_image_job().unwrap();
    v.apply_image(&decode(job));
    v.reload().unwrap();
    assert!(v.image_state().is_none());
    assert!(v.take_image_job().is_some());
}
