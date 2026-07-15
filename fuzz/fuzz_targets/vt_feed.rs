#![no_main]

use libfuzzer_sys::fuzz_target;
use seance_vt::test_support::HeadlessTerminal;

fuzz_target!(|data: &[u8]| {
    let [c, r, bytes @ ..] = data else { return };
    let cols = 1 + u16::from(*c) % 300;
    let rows = 1 + u16::from(*r) % 200;
    let Some(mut term) = HeadlessTerminal::new(cols, rows) else {
        return;
    };
    term.feed(bytes);
    let _ = term.take_responses();
    let _ = term.snapshot();
});
