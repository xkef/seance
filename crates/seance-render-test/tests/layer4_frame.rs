//! Layer 4 — frame assembly snapshots.
//!
//! Feeds each bundled VT fixture into a `TestWorld` and snapshots the
//! `dump_frame()` output. The grid box shows the visible layout; the
//! `cells:` block annotates non-default colors; cursor + selection
//! close each dump.

use insta::assert_snapshot;
use seance_render_test::TestWorld;

#[test]
fn empty_fixture_dumps_blank_grid() {
    let mut world = TestWorld::new(20, 3);
    world.feed_fixture("empty");
    assert_snapshot!(world.dump_frame());
}

#[test]
fn hello_world_fixture_shows_text_on_first_row() {
    let mut world = TestWorld::new(20, 3);
    world.feed_fixture("hello_world");
    assert_snapshot!(world.dump_frame());
}

#[test]
fn ansi_colors_annotate_per_cell_fg() {
    let mut world = TestWorld::new(40, 3);
    world.feed_fixture("ansi_colors");
    assert_snapshot!(world.dump_frame());
}

#[test]
fn box_drawing_renders_in_grid() {
    let mut world = TestWorld::new(10, 5);
    world.feed_fixture("box_drawing");
    assert_snapshot!(world.dump_frame());
}

#[test]
fn wide_chars_span_two_columns_each() {
    let mut world = TestWorld::new(20, 3);
    world.feed_fixture("wide_chars");
    assert_snapshot!(world.dump_frame());
}
