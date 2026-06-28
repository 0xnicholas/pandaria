# observability

Lightweight embedded metrics registry for Pandaria.

## Public API

- `MetricsRegistry` — thread-safe counter, gauge, histogram registry
- `export()` — Prometheus exposition format

## Dependencies

Only `dashmap`. No external metrics libraries.

## Usage

```rust
let registry = Arc::new(MetricsRegistry::new());
registry.increment_counter("my_counter", &[("label", "val")], 1);
registry.set_gauge("my_gauge", &[("label", "val")], 42);
let prometheus_text = registry.export();
```
