# Spec: Observability M1 — 轻量内嵌指标采集

> **日期**: 2026-06-28  
> **状态**: 设计中  
> **关联**: ADR-005（可观测性不可裁剪）

---

## 1. 目标

恢复 `observability` crate（v0.1.3 已删除），以轻量内嵌方式提供 per-tenant 指标采集与 Prometheus 格式导出，补齐 ADR-005 声明的可观测性基础能力。

### 非目标

- 不引入 OpenTelemetry SDK 或外部 collector（M1 范围外）
- 不实现分布式 tracing（Jaeger/Tempo）
- 不采集 per-session 维度指标（cardinality 爆炸）
- 不改造 `ai-provider` crate（保持其"纯通信层"边界）
- 不实现告警规则或 dashboard

---

## 2. 总体架构

### 2.1 Crate 依赖变更

```
改造前：
  api-gateway ──→ tenant ──→ agent-core
       │                         ↓
       └── /metrics (裸 gauge)

改造后：
  observability ←── agent-core   (埋点: tokens, tool calls)
       ↑
  observability ←── tenant       (埋点: session lifecycle)
       ↑
  api-gateway ──→ observability  (读取 export() 暴露 /metrics)
       │
  api-gateway ──→ tenant ──→ agent-core (不变)
```

`observability` 是一个纯数据 crate，不依赖任何其他 pandaria crate。通过 `Arc<MetricsRegistry>` 注入到各组件。

### 2.2 数据流

```
SessionActor (agent-core)
  │  each turn end: registry.inc_counter("tokens_consumed_total", tenant_id, usage.input)
  │  each tool call: registry.inc_counter("tool_calls_total", tenant_id, tool_name, "success")
  ▼
MetricsRegistry (observability)
  │  dashmap-backed, lock-free counters + histograms
  ▼
api-gateway GET /metrics
  │  registry.export() → Prometheus text format
  ▼
Prometheus-compatible scraper (外部)
```

---

## 3. Crate: `observability`

### 3.1 文件结构

```
crates/observability/
├── Cargo.toml
├── README.md
└── src/
    ├── lib.rs           # pub mod registry; pub use registry::MetricsRegistry
    ├── registry.rs      # 核心注册表
    └── layer.rs         # tracing Layer（M1 预留，空骨架）
```

### 3.2 依赖

```toml
[dependencies]
dashmap = "6"
```

零外部依赖。`dashmap` 已在 `tenant` crate 中使用，不引入新依赖品类。

### 3.3 核心 API

```rust
/// Thread-safe, lock-free metrics registry.
///
/// All methods accept `&self` (no `&mut self`) so a single `Arc<MetricsRegistry>`
/// can be shared across all components without additional locking.
pub struct MetricsRegistry { /* dashmap internals */ }

impl MetricsRegistry {
    /// Create an empty registry.
    pub fn new() -> Self;

    /// Increment a counter. Creates the metric if it doesn't exist.
    ///
    /// `labels` are key-value pairs embedded in the Prometheus metric name
    /// as label dimensions (e.g. `tenant_id="acme"`).
    pub fn increment_counter(
        &self,
        name: &str,
        labels: &[(&str, &str)],
        delta: u64,
    );

    /// Set a gauge to an absolute value. Overwrites previous value.
    pub fn set_gauge(
        &self,
        name: &str,
        labels: &[(&str, &str)],
        value: i64,
    );

    /// Record a duration observation into a histogram.
    ///
    /// Histogram buckets are pre-defined per metric name. If the metric
    /// doesn't exist, it's created with default buckets:
    /// [0.1, 0.5, 1.0, 5.0, 10.0, 30.0, 60.0] seconds.
    pub fn observe_duration(
        &self,
        name: &str,
        labels: &[(&str, &str)],
        seconds: f64,
    );

    /// Export all metrics in Prometheus exposition format.
    ///
    /// Returns text suitable for `Content-Type: text/plain; charset=utf-8`.
    /// Format: `<metric_name>{label="value",...} <value>\n`
    pub fn export(&self) -> String;
}
```

### 3.4 设计决策

| 决策 | 理由 |
|---|---|
| `dashmap` 而非 `Arc<RwLock<HashMap>>` | 写多读少场景，dashmap 分片锁性能更好；已在 tenant 中使用 |
| `&self` 方法 | 允许 `Arc<MetricsRegistry>` 共享，无需 `Mutex` 包裹 |
| labels 用 `&[(&str, &str)]` | 零分配调用，大部分调用点 labels 是编译期常量 |
| 无 metric 注册/注销 | M1 简单性优先，动态创建 metrics，不做生命周期管理 |
| `layer.rs` 预留空骨架 | 声明模块存在但不实现，避免后续重构时文件结构变动 |

---

## 4. 指标定义（M1 范围）

### 4.1 Session 指标

| 指标名 | 类型 | Labels | 说明 |
|---|---|---|---|
| `pandaria_sessions_active` | Gauge | `tenant_id` | 当前活跃 session 数 |
| `pandaria_sessions_total` | Counter | `tenant_id`, `status` | 累计 session 创建/完成/失败/过期数 |

`status` 取值：`created`, `completed`, `failed`, `expired`

### 4.2 Token 指标

| 指标名 | 类型 | Labels | 说明 |
|---|---|---|---|
| `pandaria_tokens_consumed_total` | Counter | `tenant_id`, `direction` | 累计 token 消耗 |

`direction` 取值：`input`, `output`

### 4.3 Tool Call 指标

| 指标名 | 类型 | Labels | 说明 |
|---|---|---|---|
| `pandaria_tool_calls_total` | Counter | `tenant_id`, `tool`, `status` | 累计 tool call 次数 |
| `pandaria_tool_call_duration_seconds` | Histogram | `tenant_id`, `tool` | tool call 耗时分布 |

`status` 取值：`success`, `blocked`, `error`

Histogram buckets: `[0.1, 0.5, 1.0, 5.0, 10.0, 30.0, 60.0]`

### 4.4 Prometheus 导出示例

```
# HELP pandaria_sessions_active Active sessions per tenant
# TYPE pandaria_sessions_active gauge
pandaria_sessions_active{tenant_id="acme"} 3
pandaria_sessions_active{tenant_id="globex"} 7

# HELP pandaria_sessions_total Total sessions by status
# TYPE pandaria_sessions_total counter
pandaria_sessions_total{tenant_id="acme",status="created"} 42
pandaria_sessions_total{tenant_id="acme",status="completed"} 38
pandaria_sessions_total{tenant_id="acme",status="failed"} 2

# HELP pandaria_tokens_consumed_total Total tokens consumed
# TYPE pandaria_tokens_consumed_total counter
pandaria_tokens_consumed_total{tenant_id="acme",direction="input"} 150000
pandaria_tokens_consumed_total{tenant_id="acme",direction="output"} 45000

# HELP pandaria_tool_calls_total Total tool calls by status
# TYPE pandaria_tool_calls_total counter
pandaria_tool_calls_total{tenant_id="acme",tool="read_file",status="success"} 120
pandaria_tool_calls_total{tenant_id="acme",tool="read_file",status="blocked"} 3

# HELP pandaria_tool_call_duration_seconds Tool call duration distribution
# TYPE pandaria_tool_call_duration_seconds histogram
pandaria_tool_call_duration_seconds_bucket{tenant_id="acme",tool="read_file",le="0.1"} 50
pandaria_tool_call_duration_seconds_bucket{tenant_id="acme",tool="read_file",le="0.5"} 90
pandaria_tool_call_duration_seconds_bucket{tenant_id="acme",tool="read_file",le="1"} 110
pandaria_tool_call_duration_seconds_bucket{tenant_id="acme",tool="read_file",le="5"} 118
pandaria_tool_call_duration_seconds_bucket{tenant_id="acme",tool="read_file",le="+Inf"} 120
pandaria_tool_call_duration_seconds_count{tenant_id="acme",tool="read_file"} 120
pandaria_tool_call_duration_seconds_sum{tenant_id="acme",tool="read_file"} 45.3
```

---

## 5. 集成点

### 5.1 注入路径

```
api-gateway main.rs
  │  let registry = Arc::new(MetricsRegistry::new());
  │
  ├─→ TenantManagerImpl::new(..., registry.clone())
  │     └─→ 存储为 self.metrics: Option<Arc<MetricsRegistry>>
  │         create_session() 时埋点
  │         SessionGuard drop 时埋点
  │         cleanup_expired_sessions() 时埋点
  │
  ├─→ AppState { metrics_registry: Some(registry.clone()) }
  │     └─→ routes/metrics.rs::get() 读取
  │
  └─→ SessionConfig { metrics: Some(registry.clone()) }
        └─→ SessionActor 存储为 self.metrics
              ├─→ AgentLoopConfig { metrics: Some(...) }
              │     └─→ AgentLoop::run() 每 turn 结束时埋点
              └─→ ToolExecutor::new(..., metrics)
                    └─→ execute_tool_call() 返回时埋点
```

### 5.2 修改清单

| Crate | 文件 | 变更 |
|---|---|---|
| `observability` | (新建) | 完整实现 `MetricsRegistry` |
| `agent-core` | `Cargo.toml` | 新增 `observability = { path = "../observability" }` |
| `agent-core` | `harness/session/mod.rs` | `SessionConfig` 新增 `pub metrics: Option<Arc<MetricsRegistry>>`；`SessionActor` 新增字段，组件创建时透传 |
| `agent-core` | `harness/agent_loop.rs` | `AgentLoopConfig` 新增 `pub metrics: Option<Arc<MetricsRegistry>>`；每 turn 结束后调用 `record_turn_metrics()` |
| `agent-core` | `harness/tool.rs` | `ToolExecutor::new()` 新增 `metrics: Option<Arc<MetricsRegistry>>` 参数；`execute_tool_call()` 返回前埋点 |
| `tenant` | `Cargo.toml` | 新增 `observability = { path = "../observability" }` |
| `tenant` | `manager.rs` | `TenantManagerImpl` 新增 `metrics` 字段；create/complete/fail/expire 时埋点；覆写 `active_session_counts()` |
| `tenant` | `manager.rs` | `TenantManager` trait 新增 `async fn active_session_counts(&self) -> HashMap<String, usize>` 默认方法 |
| `api-gateway` | `Cargo.toml` | 新增 `observability = { path = "../observability" }` |
| `api-gateway` | `server.rs` | `AppState` 新增 `pub metrics_registry: Option<Arc<MetricsRegistry>>` |
| `api-gateway` | `routes/metrics.rs` | 重写：优先使用 `registry.export()`，fallback 到旧裸 gauge |
| `api-gateway` | `main.rs` | 创建 `MetricsRegistry` 并注入到 `TenantManagerImpl` 和 `AppState` |

### 5.3 埋点伪代码

**Session 生命周期** (`tenant/src/manager.rs`):

`TenantManager` trait 已有 `active_session_count() -> usize` 方法，返回精确活跃数。M1 直接用它驱动 gauge。

终端状态埋点分散在三个路径：

| 路径 | status 标签 | 触发点 |
|---|---|---|
| `completed` | `complete_session()` 被调用时 | session 正常结束 |
| `failed` | `delete_session()` 被调用时（含错误清理） | session 异常终止 |
| `expired` | `cleanup_expired_sessions()` 批量删除时 | 后台过期清理 |

```rust
// tenant/src/manager.rs

impl TenantManagerImpl {
    pub async fn create_session(&self, tenant_id: &str, params: CreateSessionParams)
        -> Result<SessionInfo, TenantError>
    {
        // ... 现有逻辑 ...
        if let Some(ref m) = self.metrics {
            m.increment_counter("pandaria_sessions_total",
                &[("tenant_id", tenant_id), ("status", "created")], 1);
        }
        // ...
    }

    /// /metrics 端点调用此方法获取精确活跃数，再通过 set_gauge 写入。
    /// 不在室内方法中 increment/decrement counter 近似 gauge。
    pub fn active_session_count(&self) -> usize { /* 现有实现 */ }

    // 在 complete_session / delete_session / cleanup_expired_sessions 的
    // 相应位置添加：
    //   m.increment_counter("pandaria_sessions_total",
    //       &[("tenant_id", tid), ("status", "completed")], 1);
}
```

> **设计决策**：`pandaria_sessions_active` 不通过 increment/decrement 维护，而是在 `/metrics` 端点每次请求时调用 `active_session_count()` + `set_gauge()` 生成精确瞬时值。避免状态不一致。
>
> `TenantManager` trait 需新增方法以支持 per-tenant 分解：
>
> ```rust
> /// Returns active session counts keyed by tenant_id.
> /// Default impl delegates to active_session_count() with tenant_id="__total__".
> async fn active_session_counts(&self) -> HashMap<String, usize> {
>     let mut m = HashMap::new();
>     m.insert("__total__".into(), self.active_session_count());
>     m
> }
> ```
>
> `TenantManagerImpl` 覆写此方法，从 `TenantRegistry` 遍历租户获取各自计数。

**Token 消耗** (`agent-core/src/harness/agent_loop.rs`):

```rust
impl AgentLoop {
    async fn record_turn_metrics(&self, usage: &Usage) {
        if let Some(ref m) = self.config.metrics {
            let tid = &self.config.tenant_id;
            m.increment_counter("pandaria_tokens_consumed_total",
                &[("tenant_id", tid), ("direction", "input")], usage.input_tokens);
            m.increment_counter("pandaria_tokens_consumed_total",
                &[("tenant_id", tid), ("direction", "output")], usage.output_tokens);
        }
    }
}
```

**Tool Call** (`agent-core/src/harness/tool.rs`):

```rust
impl ToolExecutor {
    pub(crate) async fn execute_tool_call(&self, tool_call: &ToolCall, ...)
        -> Result<ToolResultMsg, AgentError>
    {
        // Step 1: on_tool_call (blocking hook)
        let (decision, mutation) = with_timeout_from(
            &*self.hook_dispatcher,
            self.hook_dispatcher.on_tool_call(&tool_call_ctx),
            (HookDecision::Continue, ToolCallMutation::default()),
            "on_tool_call",
        ).await;

        // ★ blocked 埋点：hook 决定阻止时立即记录
        match decision {
            HookDecision::Block { reason } => {
                if let Some(ref m) = self.metrics {
                    m.increment_counter("pandaria_tool_calls_total",
                        &[("tenant_id", &self.tenant_id),
                          ("tool", &tool_call.name),
                          ("status", "blocked")], 1);
                }
                // ... 现有 warn! 和返回 AgentError::ToolBlocked 逻辑 ...
            }
            HookDecision::Continue => { /* proceed */ }
        }

        // ... 现有执行 pipeline ...
        let start = Instant::now();
        // ... execute tool ...
        let elapsed = start.elapsed();

        if let Some(ref m) = self.metrics {
            let tid = &self.tenant_id;
            let tool = &tool_call.name;
            let status = if result.is_ok() { "success" } else { "error" };
            m.increment_counter("pandaria_tool_calls_total",
                &[("tenant_id", tid), ("tool", tool), ("status", status)], 1);
            m.observe_duration("pandaria_tool_call_duration_seconds",
                &[("tenant_id", tid), ("tool", tool)], elapsed.as_secs_f64());
        }
        result
    }
}
```

---

## 6. `/metrics` 端点

### 6.1 改造后

```rust
// api-gateway/src/routes/metrics.rs
use axum::response::IntoResponse;
use std::sync::Arc;
use crate::server::AppState;

pub async fn get(
    state: axum::extract::State<Arc<AppState>>,
) -> impl IntoResponse {
    if let Some(ref registry) = state.metrics_registry {
        // ★ 先写入 per-tenant 活跃 session gauge（每次 scrape 时取瞬时值）
        if let Ok(counts) = state.tenant_manager.active_session_counts().await {
            for (tenant_id, count) in &counts {
                registry.set_gauge(
                    "pandaria_sessions_active",
                    &[("tenant_id", tenant_id)],
                    *count as i64,
                );
            }
        }
        let body = registry.export();
        return ([("content-type", "text/plain; charset=utf-8")], body);
    }

    // Fallback: 兼容未配置 registry 的场景（测试环境 / 最小部署）
    let active = state.tenant_manager.active_session_count();
    let body = format!(
        "# HELP pandaria_active_sessions Active sessions\n\
         # TYPE pandaria_active_sessions gauge\n\
         pandaria_active_sessions {}\n",
        active
    );
    ([("content-type", "text/plain; charset=utf-8")], body)
}
```

### 6.2 AppState 新增字段

```rust
pub struct AppState {
    pub tenant_manager: Arc<dyn TenantManager>,
    pub metrics_registry: Option<Arc<observability::MetricsRegistry>>,
    // ... 其他字段不变
}
```

---

## 7. 向后兼容

- `MetricsRegistry` 为 `Option`，`None` 时所有埋点自动跳过（`if let Some` 守卫），零开销
- 未配置 registry 时 `/metrics` 回退到旧行为，不 breaking
- `SessionConfig`、`AgentLoopConfig`、`ToolExecutor` 新增字段均有默认值 `None`
- 所有现有测试无需修改（不注入 registry 时行为不变）

---

## 8. 测试策略

### 8.1 单元测试（`crates/observability/`）

| 测试 | 验证内容 |
|---|---|
| `test_counter_increment` | `increment_counter` 多次调用累加正确 |
| `test_counter_multi_label` | 不同 label 组合独立计数 |
| `test_gauge_set` | `set_gauge` 覆盖旧值 |
| `test_histogram_observe` | `observe_duration` 更新 bucket/count/sum |
| `test_export_empty` | 空 registry 导出空字符串（非 panic） |
| `test_export_format` | 导出格式符合 Prometheus text format |
| `test_concurrent_access` | 100 线程并发 increment，最终计数正确 |

### 8.2 集成测试（`crates/agent-core/tests/`）

| 测试 | 验证内容 |
|---|---|
| `test_metrics_session_lifecycle` | 创建→完成 session，验证 counter |
| `test_metrics_token_accumulation` | 多 turn 后 token counter 累加正确 |
| `test_metrics_tool_call` | tool call 成功/阻塞/错误三种状态计数 |
| `test_metrics_disabled` | registry=None 时无 panic，不影响正常流程 |

### 8.3 E2E 测试（`crates/api-gateway/tests/e2e/`）

| 测试 | 验证内容 |
|---|---|
| `e2e_metrics_endpoint` | `GET /metrics` 返回 200 + Prometheus 格式 |
| `e2e_metrics_after_session` | 创建 session 后 metrics 包含对应 label |
| `e2e_metrics_multi_tenant` | 两个 tenant 指标独立不混淆 |

---

## 9. 迁移路径

```
Phase 1 (当前 M1):
  └── MetricsRegistry + 显式埋点 + /metrics 增强

Phase 2 (未来 M2):
  ├── tracing Layer 自动采集 span 耗时/错误率
  ├── LLM API 延迟/重试指标
  ├── Hook 执行耗时指标
  └── Circuit breaker 状态 gauge

Phase 3 (未来 M3):
  └── OpenTelemetry 集成（可选，按需）
```

---

## 10. 风险评估

| 风险 | 缓解 |
|---|---|
| dashmap 内存无限增长（每个新 tool name 创建新 entry） | M1 不做清理；M2 加 `max_cardinality` 限制 + LRU 淘汰 |
| Prometheus 格式手写可能有 edge case | 单元测试覆盖特殊字符（引号、换行）的 label value |

## 11. 兼容性说明

| 变更 | 影响 |
|---|---|
| 指标名 `pandaria_active_sessions` → `pandaria_sessions_active` | **Breaking**: 升级后 Prometheus 查询和 dashboard 需更新 metric name。旧名在 registry=None 的 fallback 路径中保留，但启用 registry 后即使用新名 |
| 新增 tenant label 维度 | 现有查询 `pandaria_active_sessions` 升级后变为 `pandaria_sessions_active{tenant_id="..."}`。多租户部署需按 tenant 聚合或过滤 |
| `/metrics` 端点行为 | 兼容：未配置 registry 时行为不变。配置后输出内容增加，但仍是合法 Prometheus 格式 |

---

*本 spec 随 M1 实施更新。AGENTS.md "当前状态" 表中 `observability crate` 行从 `❌ 已删除` 更新为 `🟡 M1 重新实现中`。*
