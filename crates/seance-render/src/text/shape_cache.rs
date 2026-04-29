//! Bucketed-LRU cache for [`TextBackend::shape_cell`] output.
//!
//! [`TextBackend::shape_cell`]: super::backend::TextBackend::shape_cell
//!
//! Today every non-empty cell is independently shaped: a fresh
//! [`cosmic_text::Buffer`] is allocated and a `shape_until_scroll` pass
//! runs every frame, regardless of whether the cell changed. On an idle
//! 80×24 neovim screen that is ~1900 redundant shape passes per frame.
//!
//! [`cosmic_text::Buffer`]: cosmic_text::Buffer
//!
//! This cache memoizes shape output keyed by `(font flags, text bytes)`:
//!
//! - 256 buckets, 8 slots per bucket. Bucket index = `hash & 0xff`.
//! - Per-cache monotonic generation counter for LRU; on miss with a full
//!   bucket the lowest-generation slot is evicted.
//! - Total capacity is 2048 entries — comfortable for typical terminal
//!   working sets (~300–400 unique cells × style flags).
//! - Inline key storage of [`KEY_INLINE_BYTES`] bytes. Keys longer than
//!   that bypass the cache and call the backend directly; this is rare
//!   for terminal cells (single grapheme) but happens for unusually
//!   long ZWJ emoji sequences.
//!
//! ## Why fg/bg are NOT in the key
//!
//! Issue #21 spec'd a key including fg/bg colors. We deliberately omit
//! them: color is applied **after** shaping in
//! [`super::cell_builder`] (the `req.fg` field is baked into
//! `CellText.color` once `shape_cell` returns), so shaping itself is
//! color-agnostic. Including fg/bg would multiply key cardinality by
//! ~256³ for truecolor content with no correctness benefit, collapsing
//! hit rate on workloads like the bench's `ansi-rainbow` pass.

use std::hash::Hasher;

use rustc_hash::FxHasher;

use super::backend::{FontAttrs, ShapedGlyph};

const NUM_BUCKETS: usize = 256;
const WAYS: usize = 8;
pub(crate) const KEY_INLINE_BYTES: usize = 24;

const FLAG_BOLD: u8 = 0b01;
const FLAG_ITALIC: u8 = 0b10;

fn pack_flags(attrs: FontAttrs) -> u8 {
    let mut f = 0;
    if attrs.bold {
        f |= FLAG_BOLD;
    }
    if attrs.italic {
        f |= FLAG_ITALIC;
    }
    f
}

fn hash_key(flags: u8, text: &[u8]) -> u64 {
    let mut h = FxHasher::default();
    h.write_u8(flags);
    h.write(text);
    h.finish()
}

#[derive(Clone, Copy)]
struct SlotKey {
    flags: u8,
    len: u8,
    bytes: [u8; KEY_INLINE_BYTES],
}

impl SlotKey {
    fn write(flags: u8, text: &[u8]) -> Self {
        let mut bytes = [0u8; KEY_INLINE_BYTES];
        bytes[..text.len()].copy_from_slice(text);
        Self {
            flags,
            len: text.len() as u8,
            bytes,
        }
    }

    fn matches(&self, flags: u8, text: &[u8]) -> bool {
        self.flags == flags
            && usize::from(self.len) == text.len()
            && self.bytes[..text.len()] == *text
    }
}

struct Slot {
    occupied: bool,
    hash: u64,
    key: SlotKey,
    value: Vec<ShapedGlyph>,
    generation: u32,
}

impl Slot {
    fn empty() -> Self {
        Self {
            occupied: false,
            hash: 0,
            key: SlotKey {
                flags: 0,
                len: 0,
                bytes: [0; KEY_INLINE_BYTES],
            },
            value: Vec::new(),
            generation: 0,
        }
    }
}

struct Bucket {
    slots: Box<[Slot]>,
}

impl Bucket {
    fn new(ways: usize) -> Self {
        let slots: Vec<Slot> = (0..ways).map(|_| Slot::empty()).collect();
        Self {
            slots: slots.into_boxed_slice(),
        }
    }

    fn clear(&mut self) {
        for slot in self.slots.iter_mut() {
            slot.occupied = false;
            slot.value.clear();
            slot.generation = 0;
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub inserts: u64,
    pub evictions: u64,
    /// Lookups that bypassed the cache because the key exceeded
    /// [`KEY_INLINE_BYTES`]. Watching this should stay near zero on
    /// realistic content.
    pub bypass: u64,
}

pub(crate) struct ShapeCache {
    buckets: Box<[Bucket]>,
    stats: CacheStats,
    next_gen: u32,
    bucket_mask: usize,
}

impl ShapeCache {
    pub fn new() -> Self {
        Self::with_capacity(NUM_BUCKETS, WAYS)
    }

    fn with_capacity(buckets: usize, ways: usize) -> Self {
        assert!(
            buckets.is_power_of_two(),
            "bucket count must be a power of two"
        );
        assert!(ways > 0, "ways must be > 0");
        let bucket_array: Vec<Bucket> = (0..buckets).map(|_| Bucket::new(ways)).collect();
        Self {
            buckets: bucket_array.into_boxed_slice(),
            stats: CacheStats::default(),
            next_gen: 1,
            bucket_mask: buckets - 1,
        }
    }

    /// Drop all entries and reset stats. Called from
    /// `CellBuilder::reset_glyphs` when font size, scale, or any other
    /// shaping-state changes. Stats are zeroed so post-clear hit-rate
    /// queries reflect only the new generation of contents.
    pub fn clear(&mut self) {
        for bucket in &mut self.buckets {
            bucket.clear();
        }
        self.next_gen = 1;
        self.stats = CacheStats::default();
    }

    pub fn stats(&self) -> &CacheStats {
        &self.stats
    }

    /// Look up a shape result; on hit, copy the cached glyphs into
    /// `out` and return `true`. On miss (or oversized-key bypass), `out`
    /// is left untouched and the caller should run the backend.
    pub fn lookup_into(
        &mut self,
        text: &str,
        attrs: FontAttrs,
        out: &mut Vec<ShapedGlyph>,
    ) -> bool {
        let bytes = text.as_bytes();
        if bytes.len() > KEY_INLINE_BYTES {
            self.stats.bypass += 1;
            return false;
        }
        let flags = pack_flags(attrs);
        let hash = hash_key(flags, bytes);
        let bucket = &mut self.buckets[(hash as usize) & self.bucket_mask];
        for slot in bucket.slots.iter_mut() {
            if slot.occupied && slot.hash == hash && slot.key.matches(flags, bytes) {
                self.stats.hits += 1;
                let g = self.next_gen;
                self.next_gen = self.next_gen.wrapping_add(1);
                slot.generation = g;
                out.extend_from_slice(&slot.value);
                return true;
            }
        }
        self.stats.misses += 1;
        false
    }

    /// Insert a shape result. No-op for oversized keys; those paths
    /// never reach this function in normal flow because `lookup_into`
    /// returns `false` for them and the call site falls through to the
    /// backend without calling `insert`.
    pub fn insert(&mut self, text: &str, attrs: FontAttrs, value: &[ShapedGlyph]) {
        let bytes = text.as_bytes();
        if bytes.len() > KEY_INLINE_BYTES {
            return;
        }
        let flags = pack_flags(attrs);
        let hash = hash_key(flags, bytes);
        let bucket = &mut self.buckets[(hash as usize) & self.bucket_mask];

        let mut victim = 0usize;
        let mut victim_gen = u32::MAX;
        for (i, slot) in bucket.slots.iter().enumerate() {
            if !slot.occupied {
                victim = i;
                break;
            }
            if slot.generation < victim_gen {
                victim = i;
                victim_gen = slot.generation;
            }
        }

        let slot = &mut bucket.slots[victim];
        if slot.occupied {
            self.stats.evictions += 1;
        }
        let g = self.next_gen;
        self.next_gen = self.next_gen.wrapping_add(1);
        slot.occupied = true;
        slot.hash = hash;
        slot.key = SlotKey::write(flags, bytes);
        slot.value.clear();
        slot.value.extend_from_slice(value);
        slot.generation = g;
        self.stats.inserts += 1;
    }
}

impl Default for ShapeCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::backend::GlyphId;

    fn g(id: u64) -> ShapedGlyph {
        ShapedGlyph { id: GlyphId(id) }
    }

    fn attrs(bold: bool, italic: bool) -> FontAttrs {
        FontAttrs { bold, italic }
    }

    #[test]
    fn miss_then_hit() {
        let mut cache = ShapeCache::new();
        let mut out = Vec::new();

        assert!(!cache.lookup_into("A", attrs(false, false), &mut out));
        assert_eq!(cache.stats().misses, 1);
        assert!(out.is_empty());

        cache.insert("A", attrs(false, false), &[g(1)]);

        assert!(cache.lookup_into("A", attrs(false, false), &mut out));
        assert_eq!(cache.stats().hits, 1);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id.0, 1);
    }

    #[test]
    fn hit_appends_to_caller_scratch() {
        // The `lookup_into` contract is "extend `out`" — it does not
        // clear, since the caller is responsible for `scratch.clear()`
        // before the call (matching `shape_cell`'s own contract).
        let mut cache = ShapeCache::new();
        cache.insert("X", attrs(false, false), &[g(7), g(8)]);
        let mut out = vec![g(99)];
        assert!(cache.lookup_into("X", attrs(false, false), &mut out));
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].id.0, 99);
        assert_eq!(out[1].id.0, 7);
        assert_eq!(out[2].id.0, 8);
    }

    #[test]
    fn bold_and_italic_keys_are_distinct() {
        let mut cache = ShapeCache::new();
        cache.insert("a", attrs(false, false), &[g(1)]);
        cache.insert("a", attrs(true, false), &[g(2)]);
        cache.insert("a", attrs(false, true), &[g(3)]);
        cache.insert("a", attrs(true, true), &[g(4)]);

        for (flags, expected) in [
            (attrs(false, false), 1),
            (attrs(true, false), 2),
            (attrs(false, true), 3),
            (attrs(true, true), 4),
        ] {
            let mut out = Vec::new();
            assert!(cache.lookup_into("a", flags, &mut out));
            assert_eq!(out[0].id.0, expected);
        }
    }

    #[test]
    fn evicts_lowest_generation_when_bucket_full() {
        // 1 bucket, 2 ways. Insert 3 distinct keys; the first insert
        // is the lowest-generation slot and should be evicted on the
        // third.
        let mut cache = ShapeCache::with_capacity(1, 2);
        cache.insert("a", attrs(false, false), &[g(1)]);
        cache.insert("b", attrs(false, false), &[g(2)]);
        // Touch "b" so its generation is bumped past "a"'s insert gen.
        // (Both "a" and "b" landed in the only bucket. "a" gen=1, "b"
        // gen=2.)
        let mut out = Vec::new();
        assert!(cache.lookup_into("b", attrs(false, false), &mut out));
        out.clear();

        // Now insert "c". Bucket is full, evict lowest gen = "a".
        cache.insert("c", attrs(false, false), &[g(3)]);
        assert_eq!(cache.stats().evictions, 1);

        assert!(!cache.lookup_into("a", attrs(false, false), &mut out));
        assert!(cache.lookup_into("b", attrs(false, false), &mut out));
        out.clear();
        assert!(cache.lookup_into("c", attrs(false, false), &mut out));
    }

    #[test]
    fn lru_protects_recently_hit_entries() {
        // 1 bucket, 2 ways. Touch "a" between inserts to keep it warm;
        // it should survive while "b" is evicted.
        let mut cache = ShapeCache::with_capacity(1, 2);
        cache.insert("a", attrs(false, false), &[g(1)]);
        cache.insert("b", attrs(false, false), &[g(2)]);

        let mut out = Vec::new();
        assert!(cache.lookup_into("a", attrs(false, false), &mut out));
        out.clear();

        cache.insert("c", attrs(false, false), &[g(3)]);
        assert_eq!(cache.stats().evictions, 1);

        assert!(cache.lookup_into("a", attrs(false, false), &mut out));
        out.clear();
        assert!(!cache.lookup_into("b", attrs(false, false), &mut out));
        assert!(cache.lookup_into("c", attrs(false, false), &mut out));
    }

    #[test]
    fn clear_drops_all_entries_and_resets_stats() {
        let mut cache = ShapeCache::new();
        cache.insert("A", attrs(false, false), &[g(1)]);
        cache.insert("B", attrs(true, false), &[g(2)]);
        let mut out = Vec::new();
        cache.lookup_into("A", attrs(false, false), &mut out);
        out.clear();
        assert!(cache.stats().hits > 0 || cache.stats().inserts > 0);

        cache.clear();
        assert_eq!(cache.stats().hits, 0);
        assert_eq!(cache.stats().misses, 0);
        assert_eq!(cache.stats().inserts, 0);
        assert_eq!(cache.stats().evictions, 0);
        assert_eq!(cache.stats().bypass, 0);

        assert!(!cache.lookup_into("A", attrs(false, false), &mut out));
        assert!(!cache.lookup_into("B", attrs(true, false), &mut out));
    }

    #[test]
    fn stats_track_inserts_hits_misses_evictions() {
        let mut cache = ShapeCache::with_capacity(1, 1);
        let mut out = Vec::new();

        // Miss + insert: 1 miss, 1 insert, 0 evictions.
        assert!(!cache.lookup_into("a", attrs(false, false), &mut out));
        cache.insert("a", attrs(false, false), &[g(1)]);
        assert_eq!(cache.stats().misses, 1);
        assert_eq!(cache.stats().inserts, 1);
        assert_eq!(cache.stats().evictions, 0);

        // Hit.
        assert!(cache.lookup_into("a", attrs(false, false), &mut out));
        out.clear();
        assert_eq!(cache.stats().hits, 1);

        // Different key forces eviction (1-way bucket).
        assert!(!cache.lookup_into("b", attrs(false, false), &mut out));
        cache.insert("b", attrs(false, false), &[g(2)]);
        assert_eq!(cache.stats().misses, 2);
        assert_eq!(cache.stats().inserts, 2);
        assert_eq!(cache.stats().evictions, 1);
    }

    #[test]
    fn oversized_key_bypasses_cache() {
        let mut cache = ShapeCache::new();
        let big = "X".repeat(KEY_INLINE_BYTES + 1);
        let mut out = Vec::new();

        assert!(!cache.lookup_into(&big, attrs(false, false), &mut out));
        assert_eq!(cache.stats().bypass, 1);
        assert_eq!(cache.stats().misses, 0);

        // Insert is a no-op for oversized keys.
        cache.insert(&big, attrs(false, false), &[g(1)]);
        assert_eq!(cache.stats().inserts, 0);

        assert!(!cache.lookup_into(&big, attrs(false, false), &mut out));
        assert_eq!(cache.stats().bypass, 2);
    }

    #[test]
    fn keys_at_inline_capacity_still_cache() {
        let mut cache = ShapeCache::new();
        let exact = "Y".repeat(KEY_INLINE_BYTES);
        let mut out = Vec::new();

        assert!(!cache.lookup_into(&exact, attrs(false, false), &mut out));
        cache.insert(&exact, attrs(false, false), &[g(42)]);
        assert!(cache.lookup_into(&exact, attrs(false, false), &mut out));
        assert_eq!(out[0].id.0, 42);
        assert_eq!(cache.stats().bypass, 0);
    }

    #[test]
    fn empty_shape_results_round_trip() {
        // `shape_cell` returns zero glyphs for whitespace-only cells;
        // the cache must round-trip that as a hit with a zero-length
        // result (no allocation).
        let mut cache = ShapeCache::new();
        cache.insert(" ", attrs(false, false), &[]);
        let mut out = Vec::new();
        assert!(cache.lookup_into(" ", attrs(false, false), &mut out));
        assert!(out.is_empty());
        assert_eq!(cache.stats().hits, 1);
    }
}
