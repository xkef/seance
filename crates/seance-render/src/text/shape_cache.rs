//! Bucketed-LRU cache for [`TextBackend::shape_run`] output.
//!
//! [`TextBackend::shape_run`]: super::backend::TextBackend::shape_run
//!
//! Memoizes shape output keyed by `(font flags, text bytes)`:
//!
//! - 256 buckets × 8 slots per bucket. Bucket index = `hash & 0xff`.
//! - Per-cache monotonic generation counter for LRU; on miss with a full
//!   bucket the lowest-generation slot is evicted.
//! - Total capacity is 2048 entries — comfortable for typical terminal
//!   working sets (~300–400 unique runs × style flags).
//! - Run keys up to [`KEY_INLINE_BYTES`] bytes are stored inline; longer
//!   runs spill their key bytes to the heap so every run still caches,
//!   including a full-width same-style row or a long path. Matches compare
//!   the full byte string, so the stored hash never decides a hit alone.
//!
//! The key omits fg/bg because color is applied post-shape in
//! [`super::cell_builder`]: `CellText.color` is baked from `req.fg`
//! after `shape_run` returns. Shaping is color-agnostic; including
//! colors would multiply key cardinality by ~256³ for truecolor content
//! with no correctness benefit.

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

/// Run key bytes: inline for the common short-run case, spilled to the heap
/// only when a run exceeds [`KEY_INLINE_BYTES`] (a long path, a full-width
/// same-style row) so those runs cache instead of re-shaping every frame.
enum KeyBytes {
    Inline {
        len: u8,
        buf: [u8; KEY_INLINE_BYTES],
    },
    Heap(Box<[u8]>),
}

impl KeyBytes {
    fn new(text: &[u8]) -> Self {
        if text.len() <= KEY_INLINE_BYTES {
            let mut buf = [0u8; KEY_INLINE_BYTES];
            buf[..text.len()].copy_from_slice(text);
            Self::Inline {
                len: text.len() as u8,
                buf,
            }
        } else {
            Self::Heap(Box::from(text))
        }
    }

    fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Inline { len, buf } => &buf[..usize::from(*len)],
            Self::Heap(bytes) => bytes,
        }
    }
}

struct SlotKey {
    flags: u8,
    bytes: KeyBytes,
}

impl SlotKey {
    fn write(flags: u8, text: &[u8]) -> Self {
        Self {
            flags,
            bytes: KeyBytes::new(text),
        }
    }

    fn matches(&self, flags: u8, text: &[u8]) -> bool {
        self.flags == flags && self.bytes.as_bytes() == text
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
                bytes: KeyBytes::Inline {
                    len: 0,
                    buf: [0; KEY_INLINE_BYTES],
                },
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

    #[cfg(test)]
    pub fn stats(&self) -> &CacheStats {
        &self.stats
    }

    /// Look up a shape result; on hit, copy the cached glyphs into
    /// `out` and return `true`. On miss, `out` is left untouched and the
    /// caller should run the backend.
    pub fn lookup_into(
        &mut self,
        text: &str,
        attrs: FontAttrs,
        out: &mut Vec<ShapedGlyph>,
    ) -> bool {
        let bytes = text.as_bytes();
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

    /// Insert a shape result, keyed on the run's `(flags, bytes)`.
    pub fn insert(&mut self, text: &str, attrs: FontAttrs, value: &[ShapedGlyph]) {
        let bytes = text.as_bytes();
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
        ShapedGlyph {
            id: GlyphId(id),
            cluster: 0,
        }
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
        // before the call (matching `shape_run`'s own contract).
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
    fn oversized_key_caches_via_heap_spill() {
        // Runs longer than the inline key length previously bypassed the
        // cache and re-shaped every frame; they now spill their key bytes
        // to the heap and cache like any other run.
        let mut cache = ShapeCache::new();
        let big = "X".repeat(KEY_INLINE_BYTES + 1);
        let mut out = Vec::new();

        assert!(!cache.lookup_into(&big, attrs(false, false), &mut out));
        assert_eq!(cache.stats().misses, 1);

        cache.insert(&big, attrs(false, false), &[g(1)]);
        assert_eq!(cache.stats().inserts, 1);

        assert!(cache.lookup_into(&big, attrs(false, false), &mut out));
        assert_eq!(cache.stats().hits, 1);
        assert_eq!(out[0].id.0, 1);
    }

    #[test]
    fn long_run_reused_across_positions() {
        // The #205 goal for long runs: identical run text served from one
        // entry no matter where it recurs, so a `cat large.log` row of
        // repeated same-style text shapes once, not once per occurrence.
        let mut cache = ShapeCache::new();
        let run = "the quick brown fox jumps over".repeat(3);
        assert!(run.len() > KEY_INLINE_BYTES);
        let mut out = Vec::new();

        assert!(!cache.lookup_into(&run, attrs(false, false), &mut out));
        cache.insert(&run, attrs(false, false), &[g(5), g(6)]);

        for _ in 0..4 {
            out.clear();
            assert!(cache.lookup_into(&run, attrs(false, false), &mut out));
        }
        assert_eq!(cache.stats().hits, 4);
        assert_eq!(cache.stats().inserts, 1);
    }

    #[test]
    fn heap_key_and_its_inline_prefix_do_not_collide() {
        // A run longer than the inline length must not be confused with its
        // own truncation to the inline length: matches compare the full
        // byte string, not a length-capped prefix.
        let mut cache = ShapeCache::new();
        let long = "Z".repeat(KEY_INLINE_BYTES + 6);
        let prefix = "Z".repeat(KEY_INLINE_BYTES);
        let mut out = Vec::new();

        cache.insert(&long, attrs(false, false), &[g(1)]);
        cache.insert(&prefix, attrs(false, false), &[g(2)]);

        assert!(cache.lookup_into(&long, attrs(false, false), &mut out));
        assert_eq!(out[0].id.0, 1);
        out.clear();
        assert!(cache.lookup_into(&prefix, attrs(false, false), &mut out));
        assert_eq!(out[0].id.0, 2);
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
    }

    #[test]
    fn empty_shape_results_round_trip() {
        // `shape_run` returns zero glyphs for whitespace-only runs;
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
