//! Tick timing and health metrics.
//!
//! The game runs on a fixed 20 Hz clock. What matters operationally is not the
//! average but the tail: one 300 ms tick is felt by every player, while a
//! slightly raised mean is not. The metrics here therefore keep a window of
//! recent samples so percentiles can be reported, rather than a running mean
//! that hides spikes.

use std::collections::VecDeque;
use std::time::Duration;

/// Rolling tick-time statistics.
#[derive(Debug)]
pub struct TickMetrics {
    window: VecDeque<Duration>,
    capacity: usize,
    target: Duration,
    total_ticks: u64,
    overrun_ticks: u64,
}

impl TickMetrics {
    /// Keeps `capacity` recent samples. At 20 Hz, 1200 samples is one minute.
    #[must_use]
    pub fn new(target: Duration, capacity: usize) -> Self {
        assert!(capacity > 0, "window capacity must be positive");
        Self {
            window: VecDeque::with_capacity(capacity),
            capacity,
            target,
            total_ticks: 0,
            overrun_ticks: 0,
        }
    }

    /// Records how long one tick's work took.
    pub fn record(&mut self, elapsed: Duration) {
        if self.window.len() == self.capacity {
            self.window.pop_front();
        }
        self.window.push_back(elapsed);
        self.total_ticks += 1;
        if elapsed > self.target {
            self.overrun_ticks += 1;
        }
    }

    #[must_use]
    pub const fn total_ticks(&self) -> u64 {
        self.total_ticks
    }

    /// Ticks that took longer than the target duration.
    #[must_use]
    pub const fn overrun_ticks(&self) -> u64 {
        self.overrun_ticks
    }

    #[must_use]
    pub fn samples(&self) -> usize {
        self.window.len()
    }

    /// Mean milliseconds per tick over the window.
    #[must_use]
    pub fn mean_mspt(&self) -> f64 {
        if self.window.is_empty() {
            return 0.0;
        }
        let total: f64 = self.window.iter().map(Duration::as_secs_f64).sum();
        total * 1000.0 / self.window.len() as f64
    }

    #[must_use]
    pub fn max_mspt(&self) -> f64 {
        self.window
            .iter()
            .max()
            .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
    }

    /// Millisecond value at the given percentile, `p` in `0.0..=1.0`.
    #[must_use]
    pub fn percentile_mspt(&self, p: f64) -> f64 {
        if self.window.is_empty() {
            return 0.0;
        }
        let mut sorted: Vec<Duration> = self.window.iter().copied().collect();
        sorted.sort_unstable();
        let clamped = p.clamp(0.0, 1.0);
        // Nearest-rank: index of the smallest sample at or above the rank.
        let rank = (clamped * sorted.len() as f64).ceil() as usize;
        let index = rank.saturating_sub(1).min(sorted.len() - 1);
        sorted[index].as_secs_f64() * 1000.0
    }

    /// Effective ticks per second.
    ///
    /// Capped at the target rate: finishing a tick early means waiting, not
    /// running faster, so a lightly loaded server reports exactly its target.
    #[must_use]
    pub fn tps(&self) -> f64 {
        let target_tps = 1.0 / self.target.as_secs_f64();
        let mean = self.mean_mspt();
        if mean <= 0.0 {
            return target_tps;
        }
        (1000.0 / mean).min(target_tps)
    }

    /// Whether the server is currently keeping up.
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        self.mean_mspt() <= self.target.as_secs_f64() * 1000.0
    }

    /// One-line summary for logs.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "TPS {:.2} | MSPT mean {:.2} p95 {:.2} max {:.2} | {} overruns of {} ticks",
            self.tps(),
            self.mean_mspt(),
            self.percentile_mspt(0.95),
            self.max_mspt(),
            self.overrun_ticks,
            self.total_ticks,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: Duration = Duration::from_millis(50);

    fn metrics() -> TickMetrics {
        TickMetrics::new(TARGET, 100)
    }

    #[test]
    fn an_idle_server_reports_the_target_rate() {
        let m = metrics();
        assert_eq!(m.tps(), 20.0);
        assert_eq!(m.mean_mspt(), 0.0);
        assert!(m.is_healthy());
    }

    #[test]
    fn finishing_early_does_not_exceed_the_target_rate() {
        let mut m = metrics();
        for _ in 0..10 {
            m.record(Duration::from_millis(1));
        }
        // 1 ms of work would be 1000 TPS if uncapped; the clock still runs at 20.
        assert_eq!(m.tps(), 20.0);
        assert!(m.is_healthy());
        assert_eq!(m.overrun_ticks(), 0);
    }

    #[test]
    fn overrunning_ticks_drag_the_rate_down() {
        let mut m = metrics();
        for _ in 0..10 {
            m.record(Duration::from_millis(100));
        }
        assert!((m.tps() - 10.0).abs() < 0.001, "tps was {}", m.tps());
        assert!(!m.is_healthy());
        assert_eq!(m.overrun_ticks(), 10);
    }

    #[test]
    fn the_window_forgets_old_samples() {
        let mut m = TickMetrics::new(TARGET, 3);
        m.record(Duration::from_millis(100));
        m.record(Duration::from_millis(10));
        m.record(Duration::from_millis(10));
        assert!((m.mean_mspt() - 40.0).abs() < 0.001);

        // Pushes the 100 ms sample out of the window.
        m.record(Duration::from_millis(10));
        assert_eq!(m.samples(), 3);
        assert!((m.mean_mspt() - 10.0).abs() < 0.001);

        // Lifetime counters still remember it.
        assert_eq!(m.total_ticks(), 4);
        assert_eq!(m.overrun_ticks(), 1);
    }

    #[test]
    fn a_single_spike_shows_in_the_tail_not_the_mean() {
        // The reason percentiles are tracked at all: 99 fast ticks and one
        // terrible one look healthy on average.
        let mut m = metrics();
        for _ in 0..99 {
            m.record(Duration::from_millis(1));
        }
        m.record(Duration::from_millis(500));

        assert!(m.mean_mspt() < 10.0, "mean was {}", m.mean_mspt());
        assert!(m.is_healthy(), "the mean hides the spike");
        assert_eq!(m.max_mspt(), 500.0, "but the max does not");
        assert_eq!(m.overrun_ticks(), 1);
    }

    #[test]
    fn percentiles_use_nearest_rank() {
        let mut m = metrics();
        for i in 1..=100u64 {
            m.record(Duration::from_millis(i));
        }
        assert!((m.percentile_mspt(0.5) - 50.0).abs() < 0.001);
        assert!((m.percentile_mspt(0.95) - 95.0).abs() < 0.001);
        assert!((m.percentile_mspt(1.0) - 100.0).abs() < 0.001);
        // The lowest sample, not a panic.
        assert!((m.percentile_mspt(0.0) - 1.0).abs() < 0.001);
    }

    #[test]
    fn percentiles_are_clamped_to_a_valid_range() {
        let mut m = metrics();
        m.record(Duration::from_millis(5));
        assert_eq!(m.percentile_mspt(-1.0), 5.0);
        assert_eq!(m.percentile_mspt(2.0), 5.0);
    }

    #[test]
    fn empty_metrics_do_not_divide_by_zero() {
        let m = metrics();
        assert_eq!(m.percentile_mspt(0.5), 0.0);
        assert_eq!(m.max_mspt(), 0.0);
        assert_eq!(m.samples(), 0);
    }

    #[test]
    fn summary_mentions_the_headline_numbers() {
        let mut m = metrics();
        m.record(Duration::from_millis(25));
        let summary = m.summary();
        assert!(summary.contains("TPS"), "{summary}");
        assert!(summary.contains("MSPT"), "{summary}");
    }
}
