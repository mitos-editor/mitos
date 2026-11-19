//! Opt-in, process-local latency measurements shared by the editor and frontend.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        OnceLock,
    },
    time::{Duration, Instant},
};

use parking_lot::Mutex;
use serde::Serialize;

const MAX_GROUPS: usize = 256;
const BUCKETS_US: [u64; 24] = [
    10,
    25,
    50,
    100,
    250,
    500,
    1_000,
    2_000,
    4_000,
    8_000,
    12_000,
    16_000,
    24_000,
    33_000,
    50_000,
    75_000,
    100_000,
    150_000,
    250_000,
    500_000,
    1_000_000,
    2_000_000,
    5_000_000,
    u64::MAX,
];

static RECORDER: OnceLock<Recorder> = OnceLock::new();

pub fn enable() -> &'static Recorder {
    RECORDER.get_or_init(Recorder::default)
}

/// A monotonic timestamp with no clock read when recording is disabled.
pub fn start() -> Option<Instant> {
    RECORDER.get().map(|_| Instant::now())
}

#[derive(Clone, Debug, Default, Hash, Eq, PartialEq, Ord, PartialOrd, Serialize)]
pub struct Context {
    language: String,
    buffer_size: &'static str,
    semantic_tokens: bool,
}

impl Context {
    pub fn new(doc: &crate::Document, semantic_tokens: bool) -> Self {
        Self {
            language: doc.language_name().unwrap_or("plain-text").to_owned(),
            buffer_size: match doc.text().len() {
                0..=65_535 => "under_64_kib",
                65_536..=1_048_575 => "64_kib_to_1_mib",
                _ => "1_mib_and_up",
            },
            semantic_tokens,
        }
    }
}

#[derive(Default, Clone, Debug, Serialize)]
pub struct Details {
    pub handling_us: Option<u64>,
    pub preparation_us: Option<u64>,
    pub draw_us: Option<u64>,
    pub token_count: Option<usize>,
    pub visible: Option<bool>,
}

pub fn micros(duration: Duration) -> u64 {
    duration.as_micros().try_into().unwrap_or(u64::MAX)
}

pub struct Sample {
    pub started: Instant,
    pub operation: &'static str,
    pub variant: &'static str,
    pub context: Context,
}

impl Sample {
    pub fn record(self, details: Details) {
        self.record_at(Instant::now(), details);
    }

    pub fn record_at(self, finished: Instant, details: Details) {
        if let Some(recorder) = RECORDER.get() {
            let elapsed = finished.saturating_duration_since(self.started);
            recorder.record(self, elapsed, details);
        }
    }
}

#[derive(Clone, Hash, Eq, PartialEq, Ord, PartialOrd, Serialize)]
struct Group {
    operation: &'static str,
    variant: &'static str,
    visible: Option<bool>,
    #[serde(flatten)]
    context: Context,
}

#[derive(Clone, Default)]
struct Histogram {
    count: u64,
    buckets: [u64; BUCKETS_US.len()],
    max_us: u64,
    slowest: Details,
}

impl Histogram {
    fn record(&mut self, elapsed_us: u64, details: Details) {
        let bucket = BUCKETS_US.partition_point(|&bound| bound < elapsed_us);
        self.buckets[bucket] += 1;
        self.count += 1;
        if elapsed_us >= self.max_us {
            self.max_us = elapsed_us;
            self.slowest = details;
        }
    }

    fn percentile(&self, percent: u64) -> u64 {
        let rank = (self.count * percent).div_ceil(100);
        let mut seen = 0;
        for (count, upper) in self.buckets.iter().zip(BUCKETS_US) {
            seen += count;
            if seen >= rank {
                return upper.min(self.max_us);
            }
        }
        self.max_us
    }

    fn above(&self, threshold: u64) -> u64 {
        let first = BUCKETS_US.partition_point(|&bound| bound <= threshold);
        self.buckets[first..].iter().sum()
    }
}

#[derive(Default)]
pub struct Recorder {
    groups: Mutex<HashMap<Group, Histogram>>,
    omitted: AtomicU64,
}

impl Recorder {
    fn record(&self, sample: Sample, duration: Duration, details: Details) {
        let key = Group {
            operation: sample.operation,
            variant: sample.variant,
            visible: details.visible,
            context: sample.context,
        };
        let mut groups = self.groups.lock();
        let full = groups.len() == MAX_GROUPS;
        match groups.entry(key) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                entry.get_mut().record(micros(duration), details);
            }
            std::collections::hash_map::Entry::Vacant(entry) if !full => {
                entry
                    .insert(Histogram::default())
                    .record(micros(duration), details);
            }
            _ => {
                self.omitted.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub fn report(&self) -> Report {
        let groups = self.groups.lock().clone();
        let mut measurements: Vec<_> = groups
            .into_iter()
            .map(|(group, h)| Measurement {
                group,
                count: h.count,
                p50_us: h.percentile(50),
                p95_us: h.percentile(95),
                p99_us: h.percentile(99),
                above_16_ms: h.above(16_000),
                above_50_ms: h.above(50_000),
                max_us: h.max_us,
                slowest: h.slowest,
            })
            .collect();
        measurements.sort_unstable_by(|a, b| a.group.cmp(&b.group));
        Report {
            schema_version: 1,
            editor_version: loader::VERSION_AND_GIT_HASH,
            omitted_samples: self.omitted.load(Ordering::Relaxed),
            measurements,
        }
    }
}

#[derive(Serialize)]
pub struct Report {
    schema_version: u32,
    editor_version: &'static str,
    omitted_samples: u64,
    measurements: Vec<Measurement>,
}

#[derive(Serialize)]
struct Measurement {
    #[serde(flatten)]
    group: Group,
    count: u64,
    p50_us: u64,
    p95_us: u64,
    p99_us: u64,
    above_16_ms: u64,
    above_50_ms: u64,
    max_us: u64,
    slowest: Details,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_percentiles_and_thresholds_with_bounded_groups() {
        let recorder = Recorder::default();
        let record = |language: String, us| {
            recorder.record(
                Sample {
                    started: Instant::now(),
                    operation: "input_to_flush",
                    variant: "key",
                    context: Context {
                        language,
                        ..Context::default()
                    },
                },
                Duration::from_micros(us),
                Details::default(),
            );
        };
        for us in [100, 200, 16_000, 16_001, 50_000, 50_001] {
            record("test".into(), us);
        }
        for index in 0..MAX_GROUPS {
            record(index.to_string(), 100);
        }
        let report = recorder.report();
        let sample = report
            .measurements
            .iter()
            .find(|m| m.group.context.language == "test")
            .unwrap();
        assert_eq!(report.measurements.len(), MAX_GROUPS);
        assert_eq!(report.omitted_samples, 1);
        assert_eq!(
            (sample.count, sample.p50_us, sample.p95_us, sample.p99_us),
            (6, 16_000, 50_001, 50_001)
        );
        assert_eq!((sample.above_16_ms, sample.above_50_ms), (3, 1));
    }
}
