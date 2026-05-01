# extensions

Extension 系统。提供 Extension trait、Actor Mailbox + EventBus 混合 hook 运行时的模块。

## 职责

- `Extension` trait：extension 抽象接口（7 个 hook 方法 + tools 注册）
- `ExtensionActor`：每个 Extension 的独立 tokio task，panic 隔离，oneshot 超时
- `HookRouter`：实现 `agent_core::HookDispatcher` trait，路由 hook 到 ExtensionActor[] 或 EventBus
- `EventBus`：`tokio::sync::broadcast` 封装，观测型 hook 的 fire-and-forget 广播
- `builtins/`：内置 Extension 实现（audit、rate-limit、tool-guard 等，当前占位）

## 模块结构

```
src/
  host/                     # Extension 运行时基础设施
    extension.rs            #   Extension trait（ADR-002）
    hook_router.rs          #   HookRouter — 实现 HookDispatcher，路由到 Actor/EventBus
    extension_actor.rs      #   ExtensionActor — mpsc mailbox + EventBus subscriber
    event_bus.rs            #   EventBus — broadcast 封装 + spawn_listener
    mod.rs
  builtins/                 # 内置 Extension 实现
    mod.rs                  #   占位
```

## 公开接口

| 模块 | 核心导出 |
|---|---|
| `host::extension` | `Extension` trait |
| `host::hook_router` | `HookRouter`（implements `agent_core::HookDispatcher`） |
| `host::event_bus` | `EventBus<T>`、`spawn_listener` |
| `builtins` | 当前占位 |

## Hook 分发策略

| Hook | 类型 | 传输方式 | 合并策略 |
|---|---|---|---|
| `on_tool_call` | 阻断型拦截 | Actor Mailbox + oneshot | first-block-wins |
| `on_tool_result` | 链式拦截 | Actor Mailbox + oneshot | 链式叠加（每个 handler 在前一个突变上继续） |
| `on_context` | 链式拦截 | Actor Mailbox + oneshot | 链式叠加 |
| `on_turn_end` | 观测型 | EventBus broadcast | fire-and-forget |
| `on_agent_end` | 观测型 | EventBus broadcast | fire-and-forget |
| `on_session_start` | 观测型 | EventBus broadcast | fire-and-forget |

## 超时策略

- 阻断型/链式 oneshot：500ms，超时默认 `Continue`/跳过该 handler
- 观测型 spawn：100ms，超时静默丢弃
- Extension panic：JoinHandle 捕获，不传播到 agent loop

## 依赖

- `agent-core` — HookDispatcher trait、上下文类型、mutation 类型
- `llm-client` — ToolDef 类型
- `tokio` — 异步运行时、mpsc、broadcast、timeout
- `async-trait` — async trait 支持
- `tracing` — 事件日志
