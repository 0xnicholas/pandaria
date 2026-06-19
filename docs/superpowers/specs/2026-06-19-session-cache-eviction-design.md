# Session Cache 淘汰策略 — 设计文档

> **日期**: 2026-06-19  
> **状态**: 设计中  
> **关联**: ADR-005（Session 持久化）、PandariaAgentExecutor

---

## 1. 目标

为 Pandaria 的两个缓存层引入系统化的淘汰策略：

1. **PandariaAgentExecutor 的 SessionActor 缓存**：从惰性 idle-timeout 升级为 LRU + 容量上限 + 后台清理
2. **持久化层（PostgreSQL / Redis）的旧 session 清理**：按状态 + TTL 自动清理终态 session

### 非目标

- 不改变 `TenantCache`（api-gateway token 缓存）的淘汰策略（当前概率性清理已满足需求）
- 不改变 `SessionStore` 的核心读写路径
- 不引入分布式缓存（如 Redis 作为 SessionActor 缓存）
- 不实现 session 归档/导出功能

---

## 2. PandariaAgentExecutor LRU 缓存

### 2.1 现状

```rust
// 当前结构
struct CachedSession {
    actor: Arc<Mutex<SessionActor>>,
    last_used: Instant,
}

sessions: Arc<std::sync::Mutex<HashMap<String, CachedSession>>>,
session_idle_timeout: Duration, // default 5 min
```

**问题**：

- 惰性淘汰：session 仅在再次访问同一 key 时才检查过期
- 无容量上限：缓存 map 无限增长（仅 semaphore 限制并发数）
- 无后台清理：空闲 session 的 actor 及关联 tokio task 树永不释放，直到下次访问同一 key

### 2.2 目标架构

```
                    ┌─────────────────────────────┐
                    │  PandariaAgentExecutor       │
                    │                              │
  execute() ───────►│  acquire_or_create_session() │
                    │         │                    │
                    │         ▼                    │
                    │  ┌──────────────────┐        │
                    │  │  LruCache         │        │
                    │  │  cap = 16         │        │
                    │  │  (key → Cached)   │        │
                    │  └────────┬─────────┘        │
                    │           │                   │
                    │  LRU evict on insert (full)   │
                    │  + idle timeout background    │
                    │           │                   │
                    │  ┌────────▼─────────┐        │
                    │  │  Background Task  │        │
                    │  │  (every 60s)      │        │
                    │  │  flush + remove   │        │
                    │  │  idle > 5min      │        │
                    │  └──────────────────┘        │
                    └─────────────────────────────┘
```

### 2.3 淘汰策略

采用**双层淘汰**，按优先级：

1. **Idle timeout（后台）**：每 60 秒扫描，移除 idle > `session_idle_timeout`（默认 5 分钟）的 session。移除前调用 `actor.flush().await` 确保状态写回持久化层
2. **LRU 淘汰（插入时）**：当 `insert` 发现缓存已满（达到 `max_cached_sessions`），弹出最久未访问的条目。弹出前同样 `flush()`

**淘汰顺序**：后台 idle scan → LRU on insert

> **注意**：后台清理的 `flush()` 和执行路径的 `acquire_or_create_session()` 可能同时访问同一个 `SessionActor`。使用 `tokio::sync::Mutex::try_lock()` 避免死锁：如果无法立即获取锁（说明正在执行中），跳过该条目，等下一轮清理。

### 2.4 新增类型

```rust
/// LRU-aware session cache bound by capacity and idle timeout.
struct SessionCache {
    /// LRU map keyed by `role_id:model`. The `lru` crate maintains
    /// access order: each `get()` promotes the entry to MRU.
    entries: std::sync::Mutex<lru::LruCache<String, CachedSession>>,
    /// Max cached sessions (default: 16). Evicts LRU entry on insert
    /// when full.
    max_cached: usize,
    /// Sessions unused for longer than this are evicted by the
    /// background cleanup task.
    idle_timeout: std::time::Duration,
    /// Background cleanup interval (default: 60s).
    cleanup_interval: std::time::Duration,
}
```

**配置方法**（保持不变，向后兼容）：

- `with_session_idle_timeout(d)` — 已有，语义不变
- `with_max_cached_sessions(n)` — **新增**，默认 16
- `with_cleanup_interval(d)` — **新增**，默认 60s

### 2.5 后台清理任务

在 `PandariaAgentExecutor::new()` 中 spawn：

```rust
let cache = self.sessions.clone();
let timeout = self.session_idle_timeout;
let interval = self.cleanup_interval;

tokio::spawn(async move {
    loop {
        tokio::time::sleep(interval).await;
        let now = Instant::now();
        let mut expired: Vec<(String, Arc<Mutex<SessionActor>>)> = Vec::new();

        // Collect expired entries
        {
            let mut map = cache.entries.lock().expect("session cache poisoned");
            let keys: Vec<String> = map.iter()
                .filter(|(_, c)| now.duration_since(c.last_used) > timeout)
                .map(|(k, _)| k.clone())
                .collect();
            for k in keys {
                if let Some(cached) = map.pop(&k) {
                    expired.push((k, cached.actor));
                }
            }
        }

        // Flush expired actors outside the lock
        for (key, actor) in expired {
            if let Ok(mut a) = actor.try_lock() {
                if let Err(e) = a.flush().await {
                    tracing::warn!(%key, error = %e, "session cache flush failed during eviction");
                }
            }
            // If try_lock fails, actor is in use — skip, clean up next cycle
        }
    }
});
```

### 2.6 acquire_or_create_session 变更

当前的双重检查逻辑保留，但：

- `map.get(&cache_key)` → `map.get(&cache_key)` 自动提升为 MRU（lru crate 行为）
- `map.remove(&cache_key)` → `map.pop(&cache_key)` 从 LRU 中移除
- `map.insert(cache_key, ...)` → `map.put(cache_key, ...)` 触发 LRU 淘汰（满时）。淘汰的条目需 `flush()` 后丢弃

```rust
// LRU eviction on insert (when full)
if map.len() >= self.max_cached {
    if let Some((evicted_key, evicted)) = map.pop_lru() {
        // Drop the lock before flushing
        drop(map);
        if let Ok(mut actor) = evicted.actor.try_lock() {
            let _ = actor.flush().await;
        }
        // If try_lock fails, the actor is currently executing —
        // dropping without flush is acceptable since authoritative
        // state lives in PostgreSQL/Redis (persistence layer).
        // Re-acquire lock to continue
        map = self.sessions.lock().expect("...");
    }
}
```

---

## 3. 持久化层旧 Session 清理

### 3.1 新增 SessionStore 方法

```rust
/// SessionStore trait 新增方法
async fn cleanup_expired_sessions(
    &self,
    older_than: std::time::Duration,
) -> Result<u64, SessionStoreError>;
```

- **参数**：`older_than` — 只清理最后更新时间超过此值的 session
- **返回**：清理的 session 数量
- **范围**：仅 `Completed` / `Failed` 状态
- **不清理**：`Aborted`、`Paused`、`WaitingForSignal` 等非终态

> **设计决策**：此方法为**全局清理**，不接受 `tenant_id` 参数。与 `append_entries` / `load_entries` 等 per-tenant 方法不同，cleanup 由 `TenantManagerImpl` 中的**单例后台任务**执行，一次扫描所有 tenant 的过期 session。这是因为：
>
> 1. 清理是全量操作，按 tenant 逐个调用会产生 N 次 DB 查询，不如一条 SQL 高效
> 2. 不需要 per-tenant 配置差异（所有租户共享同一保留策略）
> 3. 避免与 per-tenant session quota 的语义混淆

### 3.2 各后端实现

#### PostgreSQL

```sql
DELETE FROM sessions
WHERE status IN ('completed', 'failed')
  AND updated_at < $1;
```

> 注意：不带 `tenant_id` 过滤，一次扫描所有租户。依赖 `(status, updated_at)` 联合索引确保性能。

返回删除行数。

#### Redis

使用 `SCAN` 遍历 session key（pattern: `session:{tenant_id}:*`），对每个 key 检查 `HGET status`，若为 `completed` 或 `failed` 且 `HGET updated_at` 超过阈值，则 `DEL`。

> 注意：生产环境中 session 数量可能很大，`SCAN` 每次批次限制 `COUNT 100`，避免阻塞 Redis。

#### In-Memory（测试用）

遍历内部 `HashMap`，按相同条件删除。

### 3.3 调度

清理任务在 `tenant::TenantManagerImpl` 初始化时 spawn，原因：

- `TenantManagerImpl` 是进程级单例（`api-gateway/main.rs` 中构造一次），与 `SessionStore` 共享生命周期
- 它是唯一持有 `Arc<dyn SessionStore>` 引用的组件，无需引入额外依赖注入
- 清理是存储维护操作，与 session 生命周期管理内聚

```rust
let store = self.store.clone();
let retention = Duration::from_secs(
    self.retention_days * 86400
);
let interval = Duration::from_secs(
    self.cleanup_interval_hours * 3600
);

tokio::spawn(async move {
    loop {
        tokio::time::sleep(interval).await;
        match store.cleanup_expired_sessions(retention).await {
            Ok(count) => {
                if count > 0 {
                    tracing::info!(count, "cleaned up expired sessions");
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "session cleanup failed");
            }
        }
    }
});
```

### 3.4 配置

配置通过 `HarnessConfig` 的环境变量加载（遵循 `crates/agent-core/src/harness/config.rs` 中 `from_env()` 的既有模式）：

| 环境变量 | 默认值 | 说明 | 加载位置 |
|---------|--------|------|---------|
| `PANDARIA_SESSION_RETENTION_DAYS` | 7 | 终态 session 保留天数 | `HarnessConfig::from_env()` |
| `PANDARIA_SESSION_CLEANUP_INTERVAL_HOURS` | 24 | 清理任务执行间隔 | `HarnessConfig::from_env()` |

`HarnessConfig` 新增字段：

```rust
pub struct HarnessConfig {
    // ... existing fields ...
    /// Days to retain completed/failed sessions before cleanup (default: 7).
    pub session_retention_days: u32,
    /// Hours between cleanup task executions (default: 24).
    pub session_cleanup_interval_hours: u32,
}
```

> `TenantManagerImpl` 从 `HarnessConfig` 读取这两个值，传递给后台清理任务。这样所有环境变量集中在 `HarnessConfig::from_env()` 管理，不分散在各 crate。

---

## 4. 数据流

```
Session Lifecycle:

  Create ──► Running ──► Completed ──► (7 days) ──► cleanup_expired_sessions() deletes
                 │            │
                 ▼            ▼
              Paused       Failed ──► (7 days) ──► cleanup_expired_sessions() deletes
                 │
                 ▼
              Aborted (保留，手动管理)

Cache Lifecycle:

  execute() ──► acquire_or_create_session()
                    │
                    ├─ cache hit (not expired) →  promote MRU → return
                    ├─ cache hit (expired)     →  pop + flush → create new
                    ├─ cache miss + not full   →  create new → insert
                    └─ cache miss + full       →  pop_lru + flush → create new → insert

  Background (every 60s):
      scan → idle > 5min → pop + try_lock + flush
```

---

## 5. 错误处理

| 场景 | 策略 |
|------|------|
| 后台清理 `flush()` 失败 | 记录 warning 日志，不重试（下次清理周期再试） |
| 后台清理 `try_lock()` 失败 | 跳过（session 正在使用中），下周期再清理 |
| LRU 淘汰 `flush()` 失败 | 同上 |
| PostgreSQL `DELETE` 失败 | 记录 error 日志，不阻塞主流程 |
| Redis `SCAN` 中途断连 | 记录 error，下次调度重试 |

---

## 6. 测试策略

### 单元测试

- `SessionCache`：插入满时 LRU 淘汰，get 提升 MRU，后台 idle 扫描
- `cleanup_expired_sessions`：各后端实现的基本逻辑
- 边界条件：空缓存、容量 0、timeout 0

### 集成测试

- `PandariaAgentExecutor`：多 role 并发访问，验证 LRU 淘汰不丢失状态
- PostgreSQL cleanup：`testcontainers` 启 PG，插入多条不同状态的 session，验证只删终态
- Redis cleanup：同上

### 不影响

- 现有 204 个 agent-core lib 测试
- 现有 13 个 tavern-comp 测试
- 现有 api-gateway E2E 测试

---

## 7. 依赖

- `lru` crate（轻量 LRU 实现，已有类似生态使用）
- 不引入新外部服务
- 不改变现有 trait 签名（只新增方法）

---

## 8. 风险

| 风险 | 缓解 |
|------|------|
| LRU 淘汰时 `flush()` 耗时阻塞 executor | `try_lock()` + 非阻塞 drop |
| 后台清理与正常执行竞争同一 session | `try_lock()` 跳过，避免死锁 |
| Redis `SCAN` 在生产环境扫描量大 | 限制 `COUNT 100`，分批执行 |
| PostgreSQL `DELETE` 大表扫慢 | 确保 `(status, updated_at)` 联合索引存在 |
