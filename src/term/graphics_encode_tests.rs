use super::*;

fn solid(w: u32, h: u32, rgba: [u8; 4]) -> RgbaImage {
    RgbaImage::from_pixel(w, h, image::Rgba(rgba))
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[test]
fn kitty_sends_one_placed_compressed_rgba_picture_and_asks_for_no_reply() {
    let out = text(&kitty(&solid(4, 2, [1, 2, 3, 255]), 10, 5));
    assert!(
        out.starts_with("\x1b_Ga=T,f=32,o=z,s=4,v=2,i=4242,c=10,r=5,C=1,q=2,m=0;"),
        "{out}"
    );
    assert!(out.ends_with("\x1b\\"));
    assert_eq!(
        out.matches("\x1b_G").count(),
        1,
        "small enough for one chunk"
    );
}

#[test]
fn a_big_kitty_picture_is_chunked_and_only_the_last_chunk_says_so() {
    // Noise does not compress, so this is well over one 4096-byte chunk.
    let mut state = 0x2545_f491_u32;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state.to_le_bytes()
    };
    let noise = RgbaImage::from_fn(64, 64, |_, _| image::Rgba(next()));
    let out = text(&kitty(&noise, 8, 4));
    let chunks = out.matches("\x1b_G").count();
    assert!(chunks > 1, "{chunks}");
    assert_eq!(out.matches("m=1;").count(), chunks - 1);
    assert_eq!(out.matches("m=0;").count(), 1);
    // Each payload fits the protocol's limit.
    for payload in out.split("\x1b\\").filter(|p| !p.is_empty()) {
        let data = payload.split_once(';').unwrap().1;
        assert!(data.len() <= 4096);
    }
}

#[test]
fn kitty_delete_removes_the_picture_and_its_data() {
    assert_eq!(text(&kitty_delete()), "\x1b_Ga=d,d=I,i=4242,q=2\x1b\\");
}

#[test]
fn iterm2_sends_a_png_sized_in_cells() {
    let out = text(&iterm2(&solid(3, 3, [0, 0, 0, 255]), 6, 2));
    assert!(out.starts_with("\x1b]1337;File=inline=1;size="), "{out}");
    assert!(out.contains(";width=6;height=2;preserveAspectRatio=0:"));
    assert!(out.ends_with('\x07'));
    // base64 of the PNG signature.
    assert!(out.contains(":iVBORw0KGgo"));
}

#[test]
fn sixel_has_its_raster_size_one_palette_entry_per_colour_and_one_band_per_six_rows() {
    // Two colours side by side, seven rows: two bands.
    let img = RgbaImage::from_fn(4, 7, |x, _| {
        if x < 2 {
            image::Rgba([255, 0, 0, 255])
        } else {
            image::Rgba([0, 0, 255, 255])
        }
    });
    let out = text(&sixel(&img, [0, 0, 0]));
    assert!(out.starts_with("\x1bPq\"1;1;4;7"), "{out}");
    assert!(out.ends_with("\x1b\\"));
    // Red is cube index 180, blue is 5.
    assert!(out.contains("#180;2;100;0;0"), "{out}");
    assert!(out.contains("#5;2;0;0;100"), "{out}");
    assert_eq!(out.matches('-').count(), 2, "{out}");
    // The first band, red: all six rows set (`~`) in the left two columns,
    // none in the right two (`?`).
    assert!(out.contains("#180~~??"), "{out}");
}

#[test]
fn sixel_run_length_encodes_long_runs() {
    let out = text(&sixel(&solid(10, 6, [255, 255, 255, 255]), [0, 0, 0]));
    assert!(out.contains("!10~"), "{out}");
}

#[test]
fn sixel_draws_transparency_as_the_background() {
    let out = text(&sixel(&solid(2, 1, [255, 255, 255, 0]), [0, 0, 0]));
    assert!(out.contains("#0;2;0;0;0"), "{out}");
    assert!(!out.contains("#215"), "{out}");
}

fn place(protocol: Protocol, x: u16) -> Placement {
    Placement {
        viewer: 1,
        x,
        y: 1,
        cols: 4,
        rows: 2,
        px: (4, 4),
        protocol,
    }
}

#[test]
fn the_painter_sends_a_picture_once_and_again_only_when_it_moves() {
    let img = solid(4, 4, [9, 9, 9, 255]);
    let mut painter = Painter::new();
    let mut out = Vec::new();
    painter
        .paint(
            &mut out,
            Some(place(Protocol::Kitty, 2)),
            Some(&img),
            [0, 0, 0],
        )
        .unwrap();
    let first = text(&out);
    assert!(first.starts_with("\x1b7\x1b[2;3H\x1b_Ga=T"), "{first}");
    assert!(first.ends_with("\x1b8"));

    out.clear();
    painter
        .paint(
            &mut out,
            Some(place(Protocol::Kitty, 2)),
            Some(&img),
            [0, 0, 0],
        )
        .unwrap();
    assert!(out.is_empty(), "the same frame sends nothing");

    out.clear();
    painter
        .paint(
            &mut out,
            Some(place(Protocol::Kitty, 3)),
            Some(&img),
            [0, 0, 0],
        )
        .unwrap();
    let moved = text(&out);
    assert!(
        moved.starts_with("\x1b_Ga=d"),
        "the old one goes first: {moved}"
    );
    assert!(moved.contains("a=T"));

    out.clear();
    painter.paint(&mut out, None, None, [0, 0, 0]).unwrap();
    assert_eq!(text(&out), text(&kitty_delete()));
}

#[test]
fn a_sixel_picture_going_away_or_moving_asks_for_a_full_redraw() {
    let img = solid(4, 4, [9, 9, 9, 255]);
    let mut painter = Painter::new();
    assert!(!painter.must_clear(Some(&place(Protocol::Sixel, 0))));
    painter
        .paint(
            &mut Vec::new(),
            Some(place(Protocol::Sixel, 0)),
            Some(&img),
            [0, 0, 0],
        )
        .unwrap();
    assert!(
        !painter.must_clear(Some(&place(Protocol::Sixel, 0))),
        "unchanged"
    );
    assert!(
        painter.must_clear(Some(&place(Protocol::Sixel, 1))),
        "moved"
    );
    assert!(painter.must_clear(None), "gone");

    // A kitty picture is on its own layer and is deleted, not cleared.
    let mut kitty_painter = Painter::new();
    kitty_painter
        .paint(
            &mut Vec::new(),
            Some(place(Protocol::Kitty, 0)),
            Some(&img),
            [0, 0, 0],
        )
        .unwrap();
    assert!(!kitty_painter.must_clear(None));
}

#[test]
fn nothing_is_remembered_as_shown_until_there_is_a_picture_to_show() {
    let mut painter = Painter::new();
    let mut out = Vec::new();
    painter
        .paint(&mut out, Some(place(Protocol::Sixel, 0)), None, [0, 0, 0])
        .unwrap();
    assert!(out.is_empty());
    assert!(!painter.must_clear(None));
}
