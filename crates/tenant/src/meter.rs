use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Thread-safe sliding-window meter for counting events or summing values.
pub struct SlidingWindowMeter {
    window: Duration,
    entries: Mutex<Vec<(Instant, u64)>>,
}

impl SlidingWindowMeter {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            entries: Mutex::new(Vec::new()),
        }
    }

    const MAX_ENTRIES: usize = 10_000;

    /// Record a value at the current time.
    pub fn record(&self, value: u64) {
        let mut entries = self.entries.lock().expect("meter mutex poisoned");
        let now = Instant::now();
        Self::prune(&mut entries, now, self.window);
        entries.push((now, value));

        // Capacity-triggered truncation: oldest 50% dropped when over limit.
        if entries.len() > Self::MAX_ENTRIES {
            let cutoff = entries.len() / 2;
            entries.drain(..cutoff);
        }
    }

    /// Sum of all values in the current window.
    pub fn sum(&self) -> u64 {
        let mut entries = self.entries.lock().expect("meter mutex poisoned");
        let now = Instant::now();
        Self::prune(&mut entries, now, self.window);
        entries.iter().map(|(_, v)| v).sum()
    }

    /// Count of entries in the current window.
    pub fn count(&self) -> usize {
        let mut entries = self.entries.lock().expect("meter mutex poisoned");
        let now = Instant::now();
        Self::prune(&mut entries, now, self.window);
        entries.len()
    }

    fn prune(entries: &mut Vec<(Instant, u64)>, now: Instant, window: Duration) {
        entries.retain(|(t, _)| now.duration_since(*t) < window);
    }
}

use std::sync::atomic::{AtomicU64, Ordering};

/// Per-tenant cumulative cost tracker.
pub struct CostTracker {
    media_cost_millis: AtomicU64, // 0.001 CNY precision
    llm_cost_millis: AtomicU64,
}

impl CostTracker {
    pub fn new() -> Self {
        Self {
            media_cost_millis: AtomicU64::new(0),
            llm_cost_millis: AtomicU64::new(0),
        }
    }

    pub fn record_media_call(&self, cost_cny: f64) {
        let millis = (cost_cny * 1000.0) as u64;
        self.media_cost_millis.fetch_add(millis, Ordering::Relaxed);
    }

    pub fn media_cost_cny(&self) -> f64 {
        self.media_cost_millis.load(Ordering::Relaxed) as f64 / 1000.0
    }

    pub fn record_llm_cost(&self, cost_cny: f64) {
        let millis = (cost_cny * 1000.0) as u64;
        self.llm_cost_millis.fetch_add(millis, Ordering::Relaxed);
    }

    pub fn llm_cost_cny(&self) -> f64 {
        self.llm_cost_millis.load(Ordering::Relaxed) as f64 / 1000.0
    }
}

impl Default for CostTracker {
    fn default() -> Self {
        Self::new()
    }
}
