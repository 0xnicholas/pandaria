//! Thread-safe metrics registry backed by dashmap.
//!
//! Supports counters (u64), gauges (i64), and histograms (f64 values
//! with pre-defined buckets).

use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Pre-defined histogram buckets for tool call duration (seconds).
const DEFAULT_BUCKETS: &[f64] = &[0.1, 0.5, 1.0, 5.0, 10.0, 30.0, 60.0];

/// Internal metric value variants.
enum MetricValue {
    Counter(AtomicU64),
    Gauge(Mutex<i64>),
    Histogram(HistogramInner),
}

struct HistogramInner {
    buckets: Vec<f64>,       // upper bounds
    counts: Vec<AtomicU64>,  // per-bucket count
    inf_count: AtomicU64,    // count of values exceeding all buckets (+Inf)
    sum: Mutex<f64>,         // sum of all observed values
}

impl HistogramInner {
    fn new(buckets: &[f64]) -> Self {
        Self {
            buckets: buckets.to_vec(),
            counts: buckets.iter().map(|_| AtomicU64::new(0)).collect(),
            inf_count: AtomicU64::new(0),
            sum: Mutex::new(0.0),
        }
    }

    fn record(&self, value: f64) {
        // Update sum
        if let Ok(mut s) = self.sum.lock() {
            *s += value;
        }
        // Find the right bucket
        for (i, upper) in self.buckets.iter().enumerate() {
            if value <= *upper {
                self.counts[i].fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
        // Value exceeds all defined buckets — record in +Inf
        self.inf_count.fetch_add(1, Ordering::Relaxed);
    }
}

/// Composite key for metric lookup: (name, label_string).
///
/// Labels are normalized as `key1=val1,key2=val2` for deterministic ordering.
#[derive(Hash, PartialEq, Eq)]
struct MetricKey {
    name: String,
    labels: String,  // sorted, comma-separated "key=val" pairs
}

impl MetricKey {
    fn new(name: &str, labels: &[(&str, &str)]) -> Self {
        let mut pairs: Vec<String> = labels
            .iter()
            .map(|(k, v)| format!("{}=\"{}\"", k, v))
            .collect();
        pairs.sort(); // deterministic ordering
        Self {
            name: name.to_string(),
            labels: pairs.join(","),
        }
    }
}

pub struct MetricsRegistry {
    metrics: DashMap<MetricKey, MetricValue>,
}

impl Default for MetricsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsRegistry {
    pub fn new() -> Self {
        Self {
            metrics: DashMap::new(),
        }
    }

    pub fn increment_counter(&self, name: &str, labels: &[(&str, &str)], delta: u64) {
        let key = MetricKey::new(name, labels);
        if let Some(entry) = self.metrics.get(&key) {
            match entry.value() {
                MetricValue::Counter(c) => {
                    c.fetch_add(delta, Ordering::Relaxed);
                }
                _ => {} // type mismatch: silently no-op
            }
        } else {
            self.metrics
                .insert(key, MetricValue::Counter(AtomicU64::new(delta)));
        }
    }

    pub fn set_gauge(&self, name: &str, labels: &[(&str, &str)], value: i64) {
        let key = MetricKey::new(name, labels);
        if let Some(entry) = self.metrics.get(&key) {
            match entry.value() {
                MetricValue::Gauge(g) => {
                    *g.lock().expect("gauge lock poisoned") = value;
                }
                _ => {} // type mismatch: silently no-op
            }
        } else {
            self.metrics
                .insert(key, MetricValue::Gauge(Mutex::new(value)));
        }
    }

    pub fn observe_duration(&self, name: &str, labels: &[(&str, &str)], seconds: f64) {
        let key = MetricKey::new(name, labels);
        use dashmap::mapref::entry::Entry;
        match self.metrics.entry(key) {
            Entry::Occupied(entry) => {
                if let MetricValue::Histogram(h) = entry.get() {
                    h.record(seconds);
                }
            }
            Entry::Vacant(entry) => {
                let h = HistogramInner::new(DEFAULT_BUCKETS);
                h.record(seconds);
                entry.insert(MetricValue::Histogram(h));
            }
        }
    }

    pub fn export(&self) -> String {
        // Group metrics by name for Prometheus HELP/TYPE lines
        let mut names: Vec<String> = self
            .metrics
            .iter()
            .map(|entry| entry.key().name.clone())
            .collect();
        names.sort();
        names.dedup();

        let mut output = String::new();

        for name in &names {
            // Collect all entries for this metric name
            let entries: Vec<_> = self
                .metrics
                .iter()
                .filter(|e| e.key().name == *name)
                .collect();

            // Determine type from first entry
            let (help_line, type_line) = match entries.first().map(|e| e.value()) {
                Some(MetricValue::Counter(_)) => (
                    format!("# HELP {} Auto-generated counter\n", name),
                    format!("# TYPE {} counter\n", name),
                ),
                Some(MetricValue::Gauge(_)) => (
                    format!("# HELP {} Auto-generated gauge\n", name),
                    format!("# TYPE {} gauge\n", name),
                ),
                Some(MetricValue::Histogram(_)) => (
                    format!("# HELP {} Auto-generated histogram\n", name),
                    format!("# TYPE {} histogram\n", name),
                ),
                None => continue,
            };

            output.push_str(&help_line);
            output.push_str(&type_line);

            for entry in &entries {
                let labels_str = if entry.key().labels.is_empty() {
                    String::new()
                } else {
                    format!("{{{}}}", entry.key().labels)
                };

                match entry.value() {
                    MetricValue::Counter(c) => {
                        let val = c.load(Ordering::Relaxed);
                        output.push_str(&format!(
                            "{}{} {}\n",
                            name, labels_str, val
                        ));
                    }
                    MetricValue::Gauge(g) => {
                        let val = *g.lock().expect("gauge lock poisoned");
                        output.push_str(&format!(
                            "{}{} {}\n",
                            name, labels_str, val
                        ));
                    }
                    MetricValue::Histogram(h) => {
                        let sum = *h.sum.lock().expect("histogram sum lock poisoned");
                        let raw_labels = &entry.key().labels;
                        let mut cumulative = 0u64;
                        for (i, bucket) in h.buckets.iter().enumerate() {
                            cumulative += h.counts[i].load(Ordering::Relaxed);
                            output.push_str(&format!(
                                "{}_bucket{{{},le=\"{}\"}} {}\n",
                                name, raw_labels, bucket, cumulative
                            ));
                        }
                        // +Inf bucket (includes values above all defined buckets)
                        let total: u64 = h.counts.iter().map(|c| c.load(Ordering::Relaxed)).sum::<u64>()
                            + h.inf_count.load(Ordering::Relaxed);
                        output.push_str(&format!(
                            "{}_bucket{{{},le=\"+Inf\"}} {}\n",
                            name, raw_labels, total
                        ));
                        output.push_str(&format!(
                            "{}_count{{{}}} {}\n",
                            name, raw_labels, total
                        ));
                        output.push_str(&format!(
                            "{}_sum{{{}}} {}\n",
                            name, raw_labels, sum
                        ));
                    }
                }
            }
        }

        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_counter_increment() {
        let reg = MetricsRegistry::new();
        reg.increment_counter("test_total", &[("tenant", "acme")], 1);
        reg.increment_counter("test_total", &[("tenant", "acme")], 2);
        let output = reg.export();
        assert!(output.contains("test_total{tenant=\"acme\"} 3"));
    }

    #[test]
    fn test_counter_multi_label() {
        let reg = MetricsRegistry::new();
        reg.increment_counter("test_total", &[("tenant", "a"), ("status", "ok")], 5);
        reg.increment_counter("test_total", &[("tenant", "b"), ("status", "err")], 3);
        let output = reg.export();
        assert!(output.contains("test_total{status=\"ok\",tenant=\"a\"} 5"));
        assert!(output.contains("test_total{status=\"err\",tenant=\"b\"} 3"));
    }

    #[test]
    fn test_gauge_set() {
        let reg = MetricsRegistry::new();
        reg.set_gauge("test_gauge", &[("tenant", "acme")], 42);
        reg.set_gauge("test_gauge", &[("tenant", "acme")], 99);
        let output = reg.export();
        assert!(output.contains("test_gauge{tenant=\"acme\"} 99"));
        assert!(!output.contains("42"));
    }

    #[test]
    fn test_histogram_observe() {
        let reg = MetricsRegistry::new();
        reg.observe_duration("test_seconds", &[("tenant", "acme")], 0.3);
        reg.observe_duration("test_seconds", &[("tenant", "acme")], 2.0);
        let output = reg.export();
        assert!(output.contains("test_seconds_bucket{tenant=\"acme\",le=\"0.5\"} 1"));
        assert!(output.contains("test_seconds_bucket{tenant=\"acme\",le=\"5\"} 2"));
        assert!(output.contains("test_seconds_bucket{tenant=\"acme\",le=\"+Inf\"} 2"));
        assert!(output.contains("test_seconds_count{tenant=\"acme\"} 2"));
    }

    #[test]
    fn test_export_empty() {
        let reg = MetricsRegistry::new();
        let output = reg.export();
        assert!(output.is_empty());
    }

    #[test]
    fn test_export_format_has_type_lines() {
        let reg = MetricsRegistry::new();
        reg.increment_counter("my_counter", &[], 1);
        reg.set_gauge("my_gauge", &[], 5);
        let output = reg.export();
        assert!(output.contains("# HELP my_counter"));
        assert!(output.contains("# TYPE my_counter counter"));
        assert!(output.contains("# HELP my_gauge"));
        assert!(output.contains("# TYPE my_gauge gauge"));
    }

    #[test]
    fn test_concurrent_access() {
        let reg = Arc::new(MetricsRegistry::new());
        let mut handles = vec![];
        for _ in 0..10 {
            let reg = reg.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    reg.increment_counter("concurrent_total", &[], 1);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let output = reg.export();
        assert!(output.contains("concurrent_total 1000"));
    }

    #[test]
    fn test_counter_gauge_type_isolation() {
        let reg = MetricsRegistry::new();
        reg.increment_counter("shared_name", &[], 10);
        reg.set_gauge("shared_name", &[], 99);
        let output = reg.export();
        assert!(output.contains("shared_name 10"));
        assert!(output.contains("# TYPE shared_name counter"));
    }
}
