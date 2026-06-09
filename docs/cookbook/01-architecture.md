# 第一章：Pandaria 核心架构

> **目标读者**：想理解「Pandaria 是什么、它怎么做到多租户隔离」的开发者。  
> **前提**：了解 Agent 和 LLM tool calling 的基本概念。

---

## 1.1 一句话定义

Pandaria 是**面向服务端多租户的 agent runtime & harness**（Rust + tokio 实现）。它把 AI agent 当作一个有状态的、可配额管理的服务来运行——而非单用户命令行工具。

**类比**：如果 pi（单机 agent CLI）是 `python app.py`，Pandaria 就是 Kubernetes + Python 运行时。

---

## 1.2 核心价值

| 能力 | 说明 |
|------|------|
| **进程级会话隔离** | 每个 session 是独立 tokio task 树，不共享可变状态 |
| **资源配额** | per-tenant CPU time budget、并发 session 上限、token 消耗计量 |
| **持久化** | 消息历史和 compaction 结果持久化到 PostgreSQL/Redis，服务重启后 session 可恢复 |
| **可观测性** | 所有 tracing span 携带 `tenant_id` 和 `session_id` |
| **内联 Hook 系统** | 阻断型/链式/观测型策略系统，直接函数调用（零 Actor overhead） |

---

## 1.3 架构分层

```
┌─────────────────────────────────────────────────┐
│                  api-gateway                     │
│         REST + SSE · 认证 · 限流                  │
└────────────────────┬────────────────────────────┘
                     │
┌────────────────────▼────────────────────────────┐
│                    tenant                        │
│     Tenant Scheduler · 配额管理 · Session 注册表   │
└────────────────────┬────────────────────────────┘
                     │
┌────────────────────▼────────────────────────────┐
│                  agent-core                      │
│  ┌─────────────┐ ┌──────────┐ ┌──────────────┐  │
│  │ AgentLoop   │ │  Hook    │ │ MemoryStore  │  │
│  │ ToolExecutor│ │ System   │ │ (trait)      │  │
│  │ Compaction  │ │          │ │              │  │
│  │ PromptBuilder│ │          │ │              │  │
│  └─────────────┘ └──────────┘ └──────────────┘  │
└────────────────────┬────────────────────────────┘
                     │
┌────────────────────▼────────────────────────────┐
│                 ai-provider                      │
│   Anthropic · OpenAI · Google · Mistral · ...    │
│   (纯通信层：HTTP + SSE 流式解析)                   │
└─────────────────────────────────────────────────┘
                     │
┌────────────────────▼────────────────────────────┐
│                  storage                         │
│   PostgreSQL / Redis adapter · Session 序列化     │
└─────────────────────────────────────────────────┘
```

**依赖方向严格单向**（禁止反向依赖）：

```
api-gateway → tenant → agent-core → ai-provider
                   ↘
                    storage
```

---

## 1.4 Crate 职责

### `agent-core` — 核心运行时

| 模块 | 路径 | 职责 |
|------|------|------|
| `harness/` | `agent_loop.rs`, `session_actor.rs`, `tool.rs`, `compaction.rs` | AgentLoop 驱动、SessionActor 生命周期、ToolExecutor 并行执行、CompactionActor 上下文压缩 |
| `hook/` | `dispatcher.rs`, `default_dispatcher.rs`, `context.rs`, `mutations.rs` | `HookDispatcher` trait、`DefaultHookDispatcher`（内联 audit/path_guard/tool_guard/token_budget/content_filter） |
| `memory/` | `store.rs`, `emerald.rs`, `types.rs`, `hook.rs`, `formatter.rs` | `MemoryStore` trait、`EmeraldMemoryStore` HTTP adapter、`MemoryHookDispatcher`、对话格式化 |
| `space.rs` | — | `AgentSpace` 统一目录抽象（config/cache/logs/workspaces/skills） |
| `skills/` | `scanner.rs`, `injector.rs` | Skill 扫描、加载、注入 PromptBuilder |
| `persistence/` | `store.rs` | `SessionStore` trait（持久化边界） |
| `prompt/` | `builder.rs` | `PromptBuilder`、`PromptMutation` |

### `tenant` — 多租户调度

- `TenantScheduler`：管理租户并发 session 上限
- 配额计量：token 消耗、tool call 次数、CPU time budget（规划中）
- `SessionRegistry`：session 生命周期追踪

### `ai-provider` — LLM 通信

- **纯通信层**：不负责 tenant 上下文、session 生命周期、资源配额
- 支持 Provider：Anthropic (Messages API)、OpenAI (Chat Completions)、Google (Gemini)、Mistral、DeepSeek、AWS Bedrock（feature-gated）
- 流式 SSE 解析 + 指数退避重试（最多 3 次）

### `api-gateway` — 接入层

- REST API + SSE 事件流
- HMAC-SHA256 认证
- 限流（token bucket）
- Session 持久化接入（persist store）

### `storage` — 持久化

- `SessionStore` trait
- PostgreSQL adapter（支持 auto-restore + 增量保存 `append_entries`）
- Redis adapter

---

## 1.5 Session 隔离模型

每个租户 session 是一个独立的 tokio task 树：

```
TenantSupervisor (per tenant)
  └── SessionActor (tenant_id, session_id)
        ├── AgentLoop          ← 驱动 tool use loop
        ├── ToolExecutor       ← 并行工具执行
        ├── DefaultHookDispatcher ← 内联 hook 策略（直接函数调用）
        ├── CompactionActor    ← 上下文压缩
        └── EventProcessor     ← 事件处理（tokio::mpsc）
```

**隔离保证：**
- Session 之间不共享任何可变状态
- 所有跨组件通信通过函数调用或 `tokio::mpsc`
- 禁止共享 `Arc<Mutex<_>>` 作为跨 session 状态

---

## 1.6 并发模型

| 场景 | 方式 |
|------|------|
| 所有 async 代码 | `tokio` runtime |
| CPU 密集型（大文本压缩、序列化） | `tokio::task::spawn_blocking` |
| Hook 调用 | 直接函数调用（同步，无超时边界） |
| 跨 session 隔离 | tokio task 级 |

**关键约束**：禁止 `std::thread::sleep` 等阻塞调用出现在 async 上下文中。

---

## 1.7 关键设计决策（ADR 摘要）

| ADR | 决策 | 理由 |
|-----|------|------|
| **ADR-001** | Agent Loop 基于 LLM 原生 tool calling 协议 | 无需自定义 DSL，LLM 直接输出 `ToolCall[]`，并行 tool call 支持 |
| **ADR-003** | Hook 机制为直接函数调用，非 Actor mailbox | 零 Actor overhead（无 mpsc、无 oneshot、无超时）；panic 直接暴露便于调试 |
| **ADR-004** | Session 隔离采用 tokio task 级别 | 轻量、与 async 生态一致、无需 OS 进程 |
| **ADR-005** | 多租户三个基础能力不可裁剪：资源配额、Session 持久化、可观测性 | 生产级服务端的底线要求 |

---

## 1.8 AgentSpace 统一目录

所有运行时数据统一在 `~/.pandaria/`（可通过 `PANDARIA_SPACE_ROOT` 覆盖）：

```
{pandaria_root}/
  ├── config/         # 配置文件
  ├── cache/          # LLM 响应缓存
  ├── logs/           # 文件日志
  ├── temp/           # 临时文件
  ├── skills/         # 全局 skill 定义
  └── workspaces/
        └── {tenant_id}/   # 租户级工作空间（文件沙箱）
```

**用途**：`PathGuard` 以 `workspace/{tenant_id}` 作为允许的文件访问前缀；Skills Scanner 扫描 `skills/` 目录；TUI 客户端使用 `config/tui/config.toml`。

---

## 1.9 下一步

- 理解 Agent 内部运行机制 → [第二章：Agent Loop 与 Tool Use](./02-agent-loop.md)
- 理解策略系统 → [第三章：Hook 系统](./03-hooks.md)
- 理解生态全景 → [第四章：生态项目概览](./04-ecosystem.md)
