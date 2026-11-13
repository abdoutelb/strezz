//! Mergeable request statistics.
//!
//! Every worker (native or browser) records latencies into the same fixed
//! log-scale histogram, so the coordinator can merge deltas from the whole
//! fleet by simple addition. The bucket formula is mirrored in `web/join.html`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Number of histogram buckets. Bucket 63 tops out around 55 seconds.
pub const BUCKETS: usize = 64;

/// Map a latency in milliseconds to its bucket (~19% resolution).
pub fn bucket_for(ms: f64) -> usize {
    let idx = ((ms.max(0.0) + 1.0).log2() * 4.0).floor() as usize;
    idx.min(BUCKETS - 1)
}

/// Upper bound (ms) of a bucket, used when estimating percentiles.
pub fn bucket_upper_ms(idx: usize) -> f64 {
    2f64.powf((idx as f64 + 1.0) / 4.0) - 1.0
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Stats {
    pub requests: u64,
    pub failures: u64,
    pub bytes: u64,
    /// Sum of latencies, for the mean.
    pub latency_sum_ms: f64,
    /// Latency histogram, `BUCKETS` long (may be empty when nothing was recorded).
    #[serde(default)]
    pub hist: Vec<u64>,
    /// Failure reason -> count.
    #[serde(default)]
    pub errors: BTreeMap<String, u64>,
}

impl Stats {
    pub fn record(&mut self, latency_ms: f64, bytes: u64, error: Option<String>) {
        if self.hist.len() != BUCKETS {
            self.hist.resize(BUCKETS, 0);
        }
        self.requests += 1;
        self.bytes += bytes;
        self.latency_sum_ms += latency_ms;
        self.hist[bucket_for(latency_ms)] += 1;
        if let Some(e) = error {
            self.failures += 1;
            *self.errors.entry(e).or_insert(0) += 1;
        }
    }

    pub fn merge(&mut self, other: &Stats) {
        self.requests += other.requests;
        self.failures += other.failures;
        self.bytes += other.bytes;
        self.latency_sum_ms += other.latency_sum_ms;
        if !other.hist.is_empty() {
            if self.hist.len() != BUCKETS {
                self.hist.resize(BUCKETS, 0);
            }
            for (a, b) in self.hist.iter_mut().zip(other.hist.iter()) {
                *a += b;
            }
        }
        for (k, v) in &other.errors {
            *self.errors.entry(k.clone()).or_insert(0) += v;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.requests == 0
    }

    /// Estimated latency percentile (`p` in 0..=100), in ms.
    pub fn percentile(&self, p: f64) -> f64 {
        let total: u64 = self.hist.iter().sum();
        if total == 0 {
            return 0.0;
        }
        let rank = ((p / 100.0) * total as f64).ceil().max(1.0) as u64;
        let mut seen = 0;
        for (i, n) in self.hist.iter().enumerate() {
            seen += n;
            if seen >= rank {
                return bucket_upper_ms(i);
            }
        }
        bucket_upper_ms(BUCKETS - 1)
    }

    pub fn mean_ms(&self) -> f64 {
        if self.requests == 0 {
            0.0
        } else {
            self.latency_sum_ms / self.requests as f64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_are_monotonic_and_bounded() {
        let mut last = 0;
        for ms in [0.0, 1.0, 5.0, 50.0, 500.0, 5_000.0, 50_000.0, 1e9] {
            let b = bucket_for(ms);
            assert!(b >= last && b < BUCKETS);
            assert!(ms <= bucket_upper_ms(b) || b == BUCKETS - 1);
            last = b;
        }
    }

    #[test]
    fn merge_adds_everything() {
        let mut a = Stats::default();
        a.record(10.0, 100, None);
        let mut b = Stats::default();
        b.record(20.0, 50, Some("HTTP 500".into()));
        b.record(30.0, 50, Some("HTTP 500".into()));
        a.merge(&b);
        assert_eq!(a.requests, 3);
        assert_eq!(a.failures, 2);
        assert_eq!(a.bytes, 200);
        assert_eq!(a.errors["HTTP 500"], 2);
        assert_eq!(a.hist.iter().sum::<u64>(), 3);
        assert!((a.mean_ms() - 20.0).abs() < 1e-9);
    }

    #[test]
    fn percentiles_are_close() {
        let mut s = Stats::default();
        for i in 1..=1000 {
            s.record(i as f64, 0, None);
        }
        let p50 = s.percentile(50.0);
        let p99 = s.percentile(99.0);
        assert!((500.0..625.0).contains(&p50), "p50={p50}");
        assert!((990.0..1237.5).contains(&p99), "p99={p99}");
        assert_eq!(Stats::default().percentile(99.0), 0.0);
    }
}
