use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

pub const DEFAULT_RING_CAPACITY: usize = 256;

/// Converts a [`Duration`] to whole microseconds, saturating at `u32::MAX`.
pub fn duration_to_us(duration: Duration) -> u32 {
    duration.as_micros().min(u128::from(u32::MAX)) as u32
}

/// Per-frame timing and metadata sample emitted by the Bevy planetarium thread.
#[derive(Clone, Copy, Debug)]
pub struct BevyFrameSample {
    pub seq: u64,
    pub finished_at: Instant,
    pub interval_us: u32,
    pub total_us: u32,
    pub gpu_sync_us: u32,
    pub commands_us: u32,
    pub camera_grid_us: u32,
    pub celestial_us: u32,
    pub hips_us: u32,
    pub render_us: u32,
    pub present_copy_us: u32,
    pub overlay_us: u32,
    pub width: u32,
    pub height: u32,
    pub command_count: u16,
    pub is_mount_mode: bool,
    pub resized: bool,
}

impl BevyFrameSample {
    /// CPU + GPU-submission work time excluding previous-frame GPU fence sync.
    pub fn work_us(&self) -> u32 {
        self.total_us.saturating_sub(self.gpu_sync_us)
    }
}

/// Bounded ring buffer of recent [`BevyFrameSample`] entries with monotonic sequence IDs.
pub(crate) struct BevyMetricsRing {
    samples: VecDeque<BevyFrameSample>,
    capacity: usize,
    next_seq: u64,
}

impl Default for BevyMetricsRing {
    fn default() -> Self {
        Self::new(DEFAULT_RING_CAPACITY)
    }
}

impl BevyMetricsRing {
    pub(crate) fn new(capacity: usize) -> Self {
        let cap = capacity.max(1);
        Self {
            samples: VecDeque::with_capacity(cap),
            capacity: cap,
            next_seq: 1,
        }
    }

    pub(crate) fn push(&mut self, mut sample: BevyFrameSample) -> u64 {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.saturating_add(1);
        sample.seq = seq;
        if self.samples.len() >= self.capacity {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
        seq
    }

    pub(crate) fn collect_since(&self, cursor: &mut u64, out: &mut Vec<BevyFrameSample>) {
        for sample in &self.samples {
            if sample.seq > *cursor {
                out.push(*sample);
            }
        }
        if let Some(last) = self.samples.back() {
            *cursor = (*cursor).max(last.seq);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_with_times(total_us: u32, gpu_sync_us: u32) -> BevyFrameSample {
        BevyFrameSample {
            seq: 0,
            finished_at: Instant::now(),
            interval_us: 16_667,
            total_us,
            gpu_sync_us,
            commands_us: 50,
            camera_grid_us: 300,
            celestial_us: 400,
            hips_us: 200,
            render_us: 1200,
            present_copy_us: 150,
            overlay_us: 0,
            width: 1280,
            height: 720,
            command_count: 1,
            is_mount_mode: false,
            resized: false,
        }
    }

    #[test]
    fn duration_to_us_saturates_large_values() {
        assert_eq!(duration_to_us(Duration::from_micros(12_345)), 12_345);
        assert_eq!(duration_to_us(Duration::from_secs(10_000)), u32::MAX);
    }

    #[test]
    fn sample_work_us_subtracts_gpu_sync() {
        let sample = sample_with_times(20_000, 12_500);
        assert_eq!(sample.work_us(), 7_500);

        let clamped = sample_with_times(500, 1_200);
        assert_eq!(clamped.work_us(), 0);
    }

    #[test]
    fn ring_evicts_oldest_and_collects_incrementally() {
        let mut ring = BevyMetricsRing::new(4);
        for i in 1..=6 {
            let seq = ring.push(sample_with_times(i * 1000, 0));
            assert_eq!(seq, u64::from(i));
        }

        let mut cursor = 0;
        let mut out = Vec::new();
        ring.collect_since(&mut cursor, &mut out);
        assert_eq!(cursor, 6);
        assert_eq!(
            out.iter().map(|s| s.seq).collect::<Vec<_>>(),
            vec![3, 4, 5, 6]
        );

        out.clear();
        ring.collect_since(&mut cursor, &mut out);
        assert!(out.is_empty());
        assert_eq!(cursor, 6);

        let seq7 = ring.push(sample_with_times(7000, 0));
        assert_eq!(seq7, 7);
        ring.collect_since(&mut cursor, &mut out);
        assert_eq!(cursor, 7);
        assert_eq!(out.iter().map(|s| s.seq).collect::<Vec<_>>(), vec![7]);
    }
}
