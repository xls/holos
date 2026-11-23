use super::*;

fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let owned: Vec<(String, String)> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    move |key| owned.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

#[test]
fn a_kitty_terminal_answers_both_and_says_so() {
    // kitty: the graphics reply, then Device Attributes without sixel.
    let reply = b"\x1b_Gi=31;OK\x1b\\\x1b[?62;22c";
    assert_eq!(
        parse_replies(reply),
        Some(Answer {
            kitty: true,
            sixel: false
        })
    );
}

#[test]
fn a_sixel_terminal_lists_4_in_its_attributes() {
    // foot.
    let reply = b"\x1b[?62;4;22c";
    assert_eq!(
        parse_replies(reply),
        Some(Answer {
            kitty: false,
            sixel: true
        })
    );
}

#[test]
fn a_4_inside_another_number_is_not_sixel() {
    assert_eq!(
        parse_replies(b"\x1b[?64;42;14c"),
        Some(Answer {
            kitty: false,
            sixel: false
        })
    );
}

#[test]
fn a_kitty_error_reply_is_not_support() {
    let reply = b"\x1b_Gi=31;ENOTSUPPORTED:no\x1b\\\x1b[?62c";
    assert_eq!(parse_replies(reply).map(|a| a.kitty), Some(false));
}

#[test]
fn the_answer_is_not_complete_until_device_attributes_ends() {
    assert_eq!(parse_replies(b""), None);
    assert_eq!(parse_replies(b"\x1b_Gi=31;OK\x1b\\"), None);
    assert_eq!(parse_replies(b"\x1b_Gi=31;OK\x1b\\\x1b[?62;4"), None);
}

#[test]
fn another_csi_question_reply_in_front_is_stepped_over() {
    // A keyboard-protocol reply arriving first, as it does when both are asked.
    let reply = b"\x1b[?1u\x1b[?65;4c";
    assert_eq!(parse_replies(reply).map(|a| a.sixel), Some(true));
}

#[test]
fn the_environment_names_the_terminals_it_can() {
    assert_eq!(
        from_env(env(&[("TERM", "xterm-kitty")])),
        Some(Protocol::Kitty)
    );
    assert_eq!(
        from_env(env(&[("TERM", "xterm-ghostty")])),
        Some(Protocol::Kitty)
    );
    assert_eq!(
        from_env(env(&[("TERM_PROGRAM", "WezTerm")])),
        Some(Protocol::Kitty)
    );
    assert_eq!(
        from_env(env(&[("TERM_PROGRAM", "iTerm.app")])),
        Some(Protocol::Iterm2)
    );
    assert_eq!(
        from_env(env(&[("TERM", "foot-extra")])),
        Some(Protocol::Sixel)
    );
    assert_eq!(from_env(env(&[("TERM", "xterm-256color")])), None);
    assert_eq!(from_env(env(&[])), None);
}

#[test]
fn inside_tmux_it_is_half_blocks_whatever_terminal_is_outside() {
    let inside = env(&[
        ("TMUX", "/tmp/tmux-1000/default,1,0"),
        ("KITTY_WINDOW_ID", "1"),
    ]);
    assert_eq!(from_env(inside), Some(Protocol::Halfblocks));
    assert_eq!(
        from_env(env(&[("TERM", "screen-256color")])),
        Some(Protocol::Halfblocks)
    );
}

#[test]
fn the_setting_wins_then_the_environment_then_the_answer() {
    let sixel = Some(Answer {
        kitty: false,
        sixel: true,
    });
    assert_eq!(
        resolve(GraphicsSetting::Halfblocks, Some(Protocol::Kitty), sixel),
        Protocol::Halfblocks
    );
    assert_eq!(resolve(GraphicsSetting::Off, None, None), Protocol::Off);
    assert_eq!(
        resolve(GraphicsSetting::Auto, Some(Protocol::Iterm2), sixel),
        Protocol::Iterm2
    );
    assert_eq!(resolve(GraphicsSetting::Auto, None, sixel), Protocol::Sixel);
    let both = Some(Answer {
        kitty: true,
        sixel: true,
    });
    assert_eq!(resolve(GraphicsSetting::Auto, None, both), Protocol::Kitty);
    assert_eq!(
        resolve(GraphicsSetting::Auto, None, None),
        Protocol::Halfblocks
    );
}

#[test]
fn the_override_is_read_like_the_setting() {
    assert_eq!(parse_setting(" Sixel "), Some(GraphicsSetting::Sixel));
    assert_eq!(
        parse_setting("halfblocks"),
        Some(GraphicsSetting::Halfblocks)
    );
    assert_eq!(parse_setting("pictures"), None);
}

#[test]
fn only_sixel_and_iterm2_write_into_the_cells() {
    assert!(Protocol::Sixel.writes_cells());
    assert!(Protocol::Iterm2.writes_cells());
    assert!(!Protocol::Kitty.writes_cells());
    assert!(!Protocol::Halfblocks.writes_cells());
    assert!(!Protocol::Halfblocks.is_pixels());
    assert!(Protocol::Kitty.is_pixels());
}
