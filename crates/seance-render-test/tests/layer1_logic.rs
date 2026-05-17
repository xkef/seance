//! Layer 1 — pure logic / behavioral invariants.
//!
//! Exercises the `HeadlessTerminal` surface directly. No rendering,
//! no fonts, no snapshots. Each test names one observable VT
//! behavior; a failure points at the specific invariant that regressed.

use seance_mux_client::{LinkDetector, LinkModifiers, LinkSource, LinkTarget};
use seance_protocol::frame::GridPos;
use seance_vt::test_support::HeadlessTerminal;

#[test]
fn new_terminal_reports_constructor_dimensions() {
    let term = HeadlessTerminal::new(80, 24).expect("construct 80x24");
    assert_eq!(term.cols(), 80);
    assert_eq!(term.rows(), 24);
}

#[test]
fn new_terminal_has_cursor_at_origin() {
    let term = HeadlessTerminal::new(80, 24).expect("construct");
    assert_eq!(term.cursor_pos(), (0, 0));
}

#[test]
fn new_terminal_has_visible_cursor() {
    let term = HeadlessTerminal::new(80, 24).expect("construct");
    assert!(term.is_cursor_visible());
}

#[test]
fn ascii_advances_cursor_by_one_per_char() {
    let mut term = HeadlessTerminal::new(80, 24).expect("construct");
    term.feed(b"hi");
    assert_eq!(term.cursor_pos(), (2, 0));
}

#[test]
fn wide_cjk_character_advances_cursor_by_two() {
    let mut term = HeadlessTerminal::new(80, 24).expect("construct");
    term.feed("你".as_bytes());
    assert_eq!(term.cursor_pos(), (2, 0));
}

#[test]
fn hide_cursor_sequence_toggles_visibility() {
    let mut term = HeadlessTerminal::new(80, 24).expect("construct");
    term.feed(b"\x1b[?25l");
    assert!(!term.is_cursor_visible());
    term.feed(b"\x1b[?25h");
    assert!(term.is_cursor_visible());
}

#[test]
fn cursor_invisible_in_snapshot_when_viewport_is_scrolled_into_scrollback() {
    // 3-row viewport, feed 9 lines so 6 land in the scrollback.
    let mut term = HeadlessTerminal::new(8, 3).expect("construct");
    for i in 0..9u8 {
        term.feed(format!("row{i}\r\n").as_bytes());
    }
    let bottom = term.snapshot().expect("snapshot at bottom");
    assert!(bottom.cursor.visible, "cursor visible at bottom of buffer");

    // Negative delta scrolls up. Three lines is enough to push the cursor
    // (anchored to the active screen) below the viewport.
    term.scroll_lines(-3);
    let scrolled = term.snapshot().expect("snapshot scrolled into scrollback");
    assert!(
        !scrolled.cursor.visible,
        "cursor hidden while viewport is in scrollback",
    );

    // Scrolling back down past the bottom is a no-op past the active screen,
    // so any value ≥ the prior up-scroll returns us to the bottom.
    term.scroll_lines(9);
    let back = term.snapshot().expect("snapshot after returning to bottom");
    assert!(back.cursor.visible, "cursor visible after return to bottom");
}

#[test]
fn cursor_position_sequence_moves_cursor() {
    let mut term = HeadlessTerminal::new(80, 24).expect("construct");
    // CSI Ps;Ps H — 1-based row;col; 5;10 → (col 9, row 4).
    term.feed(b"\x1b[5;10H");
    assert_eq!(term.cursor_pos(), (9, 4));
}

#[test]
fn sgr_does_not_move_cursor() {
    let mut term = HeadlessTerminal::new(80, 24).expect("construct");
    let before = term.cursor_pos();
    term.feed(b"\x1b[31m\x1b[0m");
    assert_eq!(term.cursor_pos(), before);
}

#[test]
fn crlf_moves_cursor_to_start_of_next_row() {
    let mut term = HeadlessTerminal::new(80, 24).expect("construct");
    term.feed(b"abc\r\n");
    assert_eq!(term.cursor_pos(), (0, 1));
}

#[test]
fn split_input_across_two_writes_matches_single_write() {
    let mut whole = HeadlessTerminal::new(80, 24).expect("construct");
    whole.feed(b"\x1b[31mred");

    let mut split = HeadlessTerminal::new(80, 24).expect("construct");
    split.feed(b"\x1b[31");
    split.feed(b"mred");

    assert_eq!(whole.cursor_pos(), split.cursor_pos());
}

#[test]
fn osc8_hyperlinks_survive_snapshot_extraction() {
    let mut term = HeadlessTerminal::new(20, 4).expect("construct");
    term.feed(b"\x1b]8;;https://example.com/x\x07link\x1b]8;;\x07 bare");

    let snapshot = term.snapshot().expect("snapshot");
    let run = snapshot.osc8_run_at(0, 0).expect("hyperlink run");

    assert_eq!(snapshot.hyperlinks, ["https://example.com/x"]);
    assert_eq!(snapshot.cell_text(snapshot.cell_at(0, 0).unwrap()), "l");
    assert_eq!(
        (run.row, run.start_col, run.end_col, run.url),
        (0, 0, 3, "https://example.com/x")
    );
    assert!(snapshot.osc8_run_at(4, 0).is_none());
}

#[test]
fn osc8_hyperlinks_with_id_params_survive_snapshot_extraction() {
    let mut term = HeadlessTerminal::new(20, 4).expect("construct");
    term.feed(b"\x1b]8;id=abc;https://example.com/x\x07link\x1b]8;;\x07");

    let snapshot = term.snapshot().expect("snapshot");
    let run = snapshot.osc8_run_at(0, 0).expect("hyperlink run");

    assert_eq!(run.url, "https://example.com/x");
}

#[test]
fn eza_symlink_output_uses_osc8_for_name_and_path_rule_for_target() {
    let mut term = HeadlessTerminal::new(80, 4).expect("construct");
    term.feed(
        b"\x1b]8;;file:///Users/kk/dotfiles/shell/.hushlogin\x1b\\.hushlogin\x1b]8;;\x1b\\ -> dotfiles/shell/.hushlogin",
    );

    let snapshot = term.snapshot().expect("snapshot");
    let mods = LinkModifiers {
        super_key: true,
        shift: true,
        ..LinkModifiers::default()
    };
    let detector = LinkDetector::default_ghostty_like(mods);

    let source = detector
        .link_at(&snapshot, GridPos { col: 0, row: 0 }, mods)
        .expect("OSC 8 source link");
    assert_eq!(source.source, LinkSource::Osc8);
    assert_eq!(
        source.target,
        LinkTarget::Url("file:///Users/kk/dotfiles/shell/.hushlogin".to_string())
    );

    let target = detector
        .link_at(&snapshot, GridPos { col: 14, row: 0 }, mods)
        .expect("path target link");
    assert_eq!(target.source, LinkSource::DefaultUrlPath);
    assert_eq!(
        target.target,
        LinkTarget::Path("dotfiles/shell/.hushlogin".to_string())
    );
    assert_eq!((target.range.start.col, target.range.end.col), (14, 38));
}

#[test]
fn osc7_pwd_survives_snapshot_extraction() {
    let mut term = HeadlessTerminal::new(20, 4).expect("construct");
    let pwd = std::env::temp_dir().join("seance-osc7-pwd");
    let payload = format!("\x1b]7;file://localhost{}\x1b\\", pwd.display());
    term.feed(payload.as_bytes());

    let snapshot = term.snapshot().expect("snapshot");

    assert_eq!(snapshot.pwd.as_deref(), pwd.to_str());
}
