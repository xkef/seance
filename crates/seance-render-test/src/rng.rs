//! Deterministic RNG wrapper.
//!
//! `rand_pcg::Pcg64Mcg` seeded from a fixture-derived `u64`; the
//! wrapper exists so later phases can swap implementations without
//! touching every call site.

use rand_core::{RngCore, SeedableRng};
use rand_pcg::Pcg64Mcg;

pub struct DeterministicRng(Pcg64Mcg);

impl DeterministicRng {
    pub fn new(seed: u64) -> Self {
        Self(Pcg64Mcg::seed_from_u64(seed))
    }

    pub fn next_u32(&mut self) -> u32 {
        self.0.next_u32()
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0.next_u64()
    }
}
