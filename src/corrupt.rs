/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Opt-in RTCV-style corruption of live guest allocations.

use crate::mem::{GuestUSize, Mem};
use crate::options::CorruptionOptions;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self {
            state: seed ^ 0x9E37_79B9_7F4A_7C15,
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, bound: u64) -> u64 {
        debug_assert_ne!(bound, 0);
        self.next_u64() % bound
    }
}

pub struct Corruptor {
    options: CorruptionOptions,
    rng: Rng,
    started_at: Option<Instant>,
    next_burst_at: Option<Instant>,
    total_corrupted: u64,
}

impl Default for Corruptor {
    fn default() -> Self {
        Self::new(CorruptionOptions::default())
    }
}

impl Corruptor {
    pub fn new(options: CorruptionOptions) -> Self {
        Self {
            rng: Rng::new(options.seed),
            options,
            started_at: None,
            next_burst_at: None,
            total_corrupted: 0,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.options.enabled
    }

    pub fn total_corrupted(&self) -> u64 {
        self.total_corrupted
    }

    pub fn tick(&mut self, mem: &mut Mem, guest_now: Instant) -> u32 {
        if !self.options.enabled {
            return 0;
        }
        let started_at = *self.started_at.get_or_insert(guest_now);
        if guest_now < started_at + self.options.startup_delay {
            return 0;
        }
        if self
            .next_burst_at
            .is_some_and(|deadline| guest_now < deadline)
        {
            return 0;
        }
        let before = self.total_corrupted;
        self.blast(mem);
        self.next_burst_at = Some(guest_now + self.options.cooldown);
        (self.total_corrupted - before) as u32
    }

    fn blast(&mut self, mem: &mut Mem) {
        // Address zero is a reserved null segment, not guest heap storage.
        let allocations: Vec<_> = mem
            .live_allocations()
            .into_iter()
            .filter_map(|(base, size)| {
                // Allocator also marks the guest's main thread stack and its
                // guard as used chunks. Keep this feature focused on heap
                // state, and never write into the stack or reserved null page.
                if base == 0 || base >= Mem::MAIN_THREAD_STACK_LOW_END {
                    return None;
                }
                let eligible_size = self
                    .options
                    .max_offset
                    .map_or(size, |limit| size.min(limit));
                (eligible_size > 0).then_some((base, eligible_size))
            })
            .collect();
        let Some(total_bytes) = allocations
            .iter()
            .try_fold(0u64, |total, &(_, size)| total.checked_add(u64::from(size)))
        else {
            log!("[corrupt] live allocation size overflow; skipping burst");
            return;
        };
        if total_bytes == 0 {
            return;
        }

        for _ in 0..self.options.bytes_per_burst {
            let mut selected = self.rng.below(total_bytes);
            let mut address: Option<GuestUSize> = None;
            for &(base, size) in &allocations {
                if selected < u64::from(size) {
                    address = Some(base + selected as GuestUSize);
                    break;
                }
                selected -= u64::from(size);
            }
            let Some(address) = address else {
                continue;
            };
            if mem
                .corrupt_byte(address, self.rng.next_u64() as u8)
                .is_some()
            {
                self.total_corrupted = self.total_corrupted.saturating_add(1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mem::PAGE_SIZE;

    #[test]
    fn disabled_by_default() {
        assert!(!Corruptor::default().is_enabled());
    }

    #[test]
    fn same_seed_corrupts_same_bytes() {
        let now = Instant::now();
        fn run(now: Instant) -> Vec<u8> {
            let mut mem = Mem::new();
            mem.set_null_segment_size(PAGE_SIZE);
            let ptr = mem.alloc(4096);
            mem.bytes_at_mut(ptr.cast(), 4096).fill(0xaa);
            let mut corruptor = Corruptor::new(CorruptionOptions {
                enabled: true,
                ..CorruptionOptions::default()
            });
            corruptor.tick(&mut mem, now);
            mem.bytes_at(ptr.cast::<u8>().cast_const(), 4096).to_vec()
        }
        assert_eq!(run(now), run(now));
    }

    #[test]
    fn burst_waits_for_configured_cooldown_and_respects_max_offset() {
        let mut mem = Mem::new();
        mem.set_null_segment_size(PAGE_SIZE);
        let ptr = mem.alloc(128);
        mem.bytes_at_mut(ptr.cast(), 128).fill(0xaa);
        let now = Instant::now();
        let mut corruptor = Corruptor::new(CorruptionOptions {
            enabled: true,
            cooldown: std::time::Duration::from_secs(3),
            bytes_per_burst: 32,
            max_offset: Some(8),
            ..CorruptionOptions::default()
        });
        corruptor.tick(&mut mem, now);
        assert_eq!(corruptor.total_corrupted(), 32);
        corruptor.tick(&mut mem, now + std::time::Duration::from_secs(2));
        assert_eq!(corruptor.total_corrupted(), 32);
        corruptor.tick(&mut mem, now + std::time::Duration::from_secs(3));
        assert_eq!(corruptor.total_corrupted(), 64);
        assert!(mem
            .bytes_at(ptr.cast::<u8>().cast_const(), 8)
            .iter()
            .any(|b| *b != 0xaa));
        assert!(mem
            .bytes_at((ptr.cast::<u8>().cast_const() + 8), 120)
            .iter()
            .all(|b| *b == 0xaa));
    }

    #[test]
    fn max_offset_is_clamped_to_small_allocations() {
        let mut mem = Mem::new();
        mem.set_null_segment_size(PAGE_SIZE);
        let ptr = mem.alloc(4);
        let chunk_size = mem
            .live_allocations()
            .into_iter()
            .find(|(base, _)| *base == ptr.to_bits())
            .unwrap()
            .1;
        mem.bytes_at_mut(ptr.cast(), chunk_size).fill(0xaa);
        let mut corruptor = Corruptor::new(CorruptionOptions {
            enabled: true,
            bytes_per_burst: 16,
            max_offset: Some(32),
            ..CorruptionOptions::default()
        });
        corruptor.tick(&mut mem, Instant::now());
        assert_eq!(corruptor.total_corrupted(), 16);
        assert!(mem
            .bytes_at(ptr.cast::<u8>().cast_const(), chunk_size)
            .iter()
            .any(|b| *b != 0xaa));
    }

    #[test]
    fn blast_does_not_corrupt_reserved_main_thread_stack() {
        let mut mem = Mem::new();
        mem.set_null_segment_size(PAGE_SIZE);
        let stack_start = Mem::MAIN_THREAD_STACK_LOW_END;
        mem.bytes_at_mut(crate::mem::MutPtr::from_bits(stack_start), 64)
            .fill(0xaa);
        mem.alloc(16);
        let mut corruptor = Corruptor::new(CorruptionOptions {
            enabled: true,
            bytes_per_burst: 64,
            ..CorruptionOptions::default()
        });

        corruptor.tick(&mut mem, Instant::now());

        assert!(mem
            .bytes_at(crate::mem::Ptr::<u8, false>::from_bits(stack_start), 64)
            .iter()
            .all(|byte| *byte == 0xaa));
    }

    #[test]
    fn zero_max_offset_disables_writes() {
        let mut mem = Mem::new();
        mem.set_null_segment_size(PAGE_SIZE);
        let ptr = mem.alloc(4);
        mem.bytes_at_mut(ptr.cast(), 4).fill(0xaa);
        let mut corruptor = Corruptor::new(CorruptionOptions {
            enabled: true,
            bytes_per_burst: 4,
            max_offset: Some(0),
            ..CorruptionOptions::default()
        });
        corruptor.tick(&mut mem, Instant::now());
        assert_eq!(corruptor.total_corrupted(), 0);
        assert_eq!(mem.bytes_at(ptr.cast::<u8>().cast_const(), 4), &[0xaa; 4]);
    }
}
