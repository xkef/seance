//! Layer 1 — pure logic / behavioral invariants.
//!
//! Exercises the `HeadlessTerminal` surface directly. No rendering,
//! no fonts, no snapshots. Each test names one observable VT
//! behavior; a failure points at the specific invariant that regressed.

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
