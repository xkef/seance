//! Synthetic VT byte streams modelled on the sub-issue #26 workload list.
//!
//! Each workload is a prerecorded blob of bytes a future VT-driving step
//! will feed into a headless `libghostty-vt::Terminal`. Today the harness
//! only measures byte-processing proxies — the workload shapes are chosen
//! so the byte count + dirty-row density is representative.

pub struct Workload {
    pub name: &'static str,
    pub bytes: Vec<u8>,
    /// Hint: expected dirty rows per frame if each `bytes` slice were
    /// fed frame-by-frame. Only used for diagnostic output for now.
    pub dirty_rows_hint: u16,
}

impl Workload {
    pub fn all() -> Vec<Self> {
        vec![static_ls(), random_hexdump(), scroll_yes(), ansi_rainbow()]
    }
}

/// Steady output — `ls -la`-style listing. No repaint pressure.
fn static_ls() -> Workload {
    let mut bytes = Vec::with_capacity(4096);
    bytes.extend_from_slice(b"total 128\r\n");
    for i in 0..40 {
        let line = format!(
            "-rw-r--r-- 1 user staff {:>6} Apr 19 12:{:02} file_{:03}.txt\r\n",
            1024 + i * 17,
            i % 60,
            i
        );
        bytes.extend_from_slice(line.as_bytes());
    }
    Workload {
        name: "static-ls",
        bytes,
        dirty_rows_hint: 41,
    }
}

/// High-churn output — `cat /dev/urandom | hexdump` proxy.
/// Every cell on every line changes; worst case for any per-row cache.
fn random_hexdump() -> Workload {
    use std::fmt::Write;

    let mut bytes = Vec::with_capacity(8192);
    let mut state: u32 = 0x9E37_79B9;
    for row in 0..50 {
        let mut line = format!("{:08x}  ", row * 16);
        for _ in 0..16 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let _ = write!(line, "{:02x} ", state as u8);
        }
        line.push_str("\r\n");
        bytes.extend_from_slice(line.as_bytes());
    }
    Workload {
        name: "random-hexdump",
        bytes,
        dirty_rows_hint: 50,
    }
}

/// Rapid scroll — `yes | head` proxy. Same short line repeated with \n,
/// forcing libghostty-vt's scroll path.
fn scroll_yes() -> Workload {
    let mut bytes = Vec::with_capacity(4096);
    for _ in 0..1000 {
        bytes.extend_from_slice(b"y\r\n");
    }
    Workload {
        name: "scroll-yes",
        bytes,
        dirty_rows_hint: 1,
    }
}

/// SGR color churn — stresses shape-cache keying on attribute changes.
fn ansi_rainbow() -> Workload {
    let mut bytes = Vec::with_capacity(4096);
    for row in 0..30 {
        for col in 0..80 {
            let fg = 31 + ((row + col) % 7) as u8;
            bytes.extend_from_slice(format!("\x1b[{fg}m#").as_bytes());
        }
        bytes.extend_from_slice(b"\x1b[0m\r\n");
    }
    Workload {
        name: "ansi-rainbow",
        bytes,
        dirty_rows_hint: 30,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_workload_has_bytes() {
        for w in Workload::all() {
            assert!(!w.bytes.is_empty(), "{} is empty", w.name);
            assert!(w.dirty_rows_hint > 0);
        }
    }

    #[test]
    fn names_are_unique() {
        let all = Workload::all();
        let mut names: Vec<_> = all.iter().map(|w| w.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Workload::all().len());
    }
}
