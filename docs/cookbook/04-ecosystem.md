# 第四章：生态项目概览

> **目标读者**：想理解各卫星项目解决什么问题的所有人。  
> **前提**：了解 Pandaria 的基本概念（第一章）。

---

## 4.1 生态全景

```
                          ┌─────────────────────────────┐
                          │         Aspectus            │
                          │  统一身份 · 多租户 · 认证      │
                          └──────────┬──────────────────┘
                                     │ 身份校验 / 租户解析
          ┌──────────────┬───────────┼───────────┬──────────────┐
          │              │           │           │              │
          ▼              ▼           ▼           ▼              │
   ┌────────────┐ ┌────────────┐ ┌──────────┐ ┌──────────┐      │
   │  Emerald   │ │  Pandaria  │ │ Heirloom │ │  Tavern  │      │
   │  记忆系统   │ │ Agent      │ │ 语义本体   │ │  编排     │      │
   │            │ │ Runtime    │ │ 数据OS    │ │  框架     │      │
   │            │ │ + 在线 Guard│ │           │ │          │      │
   └─────┬──────┘ └──┬───┬─────┘ └──────────┘ └────┬─────┘      │
         │           │   │                         │            │
         │           │   │ (HeirloomTool)          │            │
         │           │   └──────────────────────┐  │            │
         │           │                          │  │            │
         ▼           ▼                          ▼  ▼            │
   ┌──────────────────────────────────────────────────────────┐ │
   │                        Constell                          │ │
   │  可观测 · 追踪 · 指标 · 评测 · Prompt 管理 · 离线 Guard    │◄┘
   └──────────────────────────────────────────────────────────┘

   ┌────────────┐     ┌────────────┐
   │  Pawbun    │     │ Tokencamp  │
   │  工具库     │     │ LLM 网关    │
   └────────────┘     └────────────┘
   (Pandaria 编译期依赖)  (未来替代 ai-provider)
```

> **Heirloom 的定位**：独立的语义数据层——仅与 Pandaria 有直接关系。
> **Guard 的定位**：在线 guard 内置于 Pandaria hook，离线 guard 作为 Constell 功能模块。不设独立 Guardrails 项目。
> **Tokencamp 的定位**：未来替代 Pandaria 内嵌的 `ai-provider` crate，作为独立 LLM 网关运行。
> **Spellpaw 的定位**：Pandaria 生态的应用层消费者——不是基础设施，而是站在 Pandaria 肩膀上的端到端产品。

**九个项目**：

| 项目 | 维度 | 状态 | 一句话 |
|------|------|:--:|------|
| **Pandaria** | Runtime + 在线 Guard | ✅ | 多租户 agent 执行引擎 + 在线安全护栏 |
| **Emerald** | 记忆 | ✅ | 知识图谱——不遗忘、有时序、有个性化 |
| **Pawbun** | 工具 | ✅ | 工具箱——工具注册、MCP、多模态文件 |
| **Tavern** | 编排 | ✅ | 多 Agent 编排——DAG、事件溯源 |
| **Constell** | 可观测 + 评测 + 离线 Guard | 🟡 | 追踪、指标、Prompt 管理、LLM 评测、离线安全检测 |
| **Heirloom** | 数据语义 | 📐 | 语义本体——类型级安全、Action 九步校验 |
| **Aspectus** | 身份与多租户 | 💡 | 统一身份、认证、租户管理 |
| **Tokencamp** | LLM 网关 | 💡 | 多 Provider 统一接入、API Key 管理、token 计量 |
| **Spellpaw** | 应用（短剧创作） | 🟡 | AI 辅助短剧/短视频创作工具——Pandaria 应用消费者 |
| **Daypaw Agent Studio** | 应用（Agent 构建） | 🟡 | Agent 可视化构建与配置工作台——Pandaria 管理界面 |

---

## 4.2 Emerald — 记忆与上下文基础设施

| 维度 | 说明 |
|------|------|
| 语言 | Python |
| 核心模型 | **知识图谱**（非向量数据库） |
| 关键概念 | 事实（memory）通过关系（更新 / 扩展 / 推导）彼此连接 |
| 核心 API | `add`、`search`、`profile`、`upload` |

### 记忆 ≠ RAG

这是 Emerald 最根本的设计原则：

| | RAG | 记忆 |
|---|---|---|
| **状态** | 无状态——所有人结果相同 | 有状态——每个用户/实体各不相同 |
| **时序** | 没有时间概念 | 追踪事实何时为真、何时过期 |
| **关系** | 无 | 事实之间可以更新、扩展、推导 |
| **遗忘** | 从不遗忘 | 自动过期临时事实、解决矛盾 |

**示例**：用户第 1 天说「喜欢 Adidas」，第 30 天说「Adidas 质量差，换 Puma」。RAG 返回「喜欢 Adidas」（语义相似度最高），记忆返回「现在偏好 Puma」（追踪了时序演进和矛盾）。

### 知识图谱关系类型

```
更新 (Update)：  「在 Google 工作」→「刚加入 Stripe」
                 记忆 2 取代记忆 1（旧事实标记 isLatest=false）

扩展 (Extend)：  「在 Stripe 工作」→「领导 5 人支付团队」
                 两个事实都有效，彼此丰富

推导 (Infer)：    「月薪 3 万」+「生活费 8000」→「每月可存 2.2 万」
                 系统推断用户未明确陈述的事实
```

### 与 Pandaria 的集成

Pandaria 通过 `EmeraldMemoryStore` HTTP adapter 调 Emerald API。详见 [第五章](./05-integration.md#51-emerald-记忆系统)。

---

## 4.3 Pawbun — 工具 & 多模态库

| 维度 | 说明 |
|------|------|
| 语言 | Rust |
| 设计参考 | CrewAI Tools + CrewAI Files |
| 核心 crate | 5 个 |

### Crate 结构

| Crate | 用途 | 测试数 |
|-------|------|:---:|
| `pawbun-toolkit` | `Tool` / `AsyncTool` trait、`ToolKit` 注册中心、内置工具（文件读写、Web 搜索/抓取、CSV/JSON 查询）、MCP 客户端 | 核心模块全覆盖 |
| `pawbun-toolkit-macros` | `#[pawbun_tool]` 属性宏，自动生成样板代码 | — |
| `pawbun-files` | 多模态文件处理：本地/URL/字节加载、OpenAI / Anthropic / Gemini / Azure 格式化 | 全覆盖 |
| `pawbun-mcp-core` | MCP 协议类型：JSON-RPC 2.0、`ToolParameter`、`Transport` trait | 全覆盖 |
| `pawbun-mcp-server` | MCP 服务器：stdio / SSE 传输、配置化 Builder、工具桥接 | 全覆盖 |

### 内置工具

| 工具 | 功能 | Feature |
|------|------|:---:|
| `file_read` | 读取文件（沙箱路径 + 大小限制） | — |
| `file_write` | 写入文件（自动创建目录、TOCTOU 防护） | — |
| `directory_list` | 列出目录内容 | — |
| `web_fetch` | HTTP 抓取页面 | `http` |
| `web_search` | 调用搜索 API | `http` |
| `csv_query` | CSV 数据查询 | `csv` |
| `json_query` | JSONPath 查询 | `jsonpath` |
| `code_execute` | 代码执行（占位，可通过 Docker 适配器接入） | — |
| `embedding` | 文本嵌入（占位，可通过 OpenAI 适配器接入） | — |
| `vision` | 视觉分析（占位，可通过 OpenAI 适配器接入） | — |

### MCP 支持

Pawbun 提供完整的 MCP (Model Context Protocol) 栈：
- **客户端**：`StdioTransport` / `SseTransport`，`DynamicTool` 代理远程工具
- **服务端**：`McpServer` + `McpServerBuilder`，stdio / SSE 传输
- **安全**：SSRF 防护、路径遍历防护

### 与 Pandaria 的集成

通过 `PawbunToolAdapter` 将 Pawbun 工具适配为 Pandaria 的 `AgentTool` trait。详见 [第五章](./05-integration.md#52-pawbun-工具系统)。

---

## 4.4 Tavern — 多 Agent 编排框架

| 维度 | 说明 |
|------|------|
| 语言 | Rust |
| 设计参考 | LangGraph（概念层） |
| 核心 crate | 8 个 |

### Crate 结构

| Crate | 用途 |
|-------|------|
| `tavern-core` | 共享类型：`AgentConfig`、`ModelConfig`、`Runtime` trait |
| `tavern-adapters` | Pandaria HTTP adapter + Mock adapter |
| `tavern-hero` | Agent 注册表、YAML 配置加载、任务分发 |
| `tavern-comp` | 事件溯源 Workflow 引擎：DAG 调度、重试/信号/超时 |
| `tavern-flow` | 方法级事件驱动编排：`#[start]` / `#[listen]` / `#[router]` |
| `tavern-flow-macros` | 过程宏：`#[derive(Flow)]`、`#[flow_impl]` |
| `tavern-config` | figment 统一配置（TOML + 环境变量） |
| `tavern-server` | HTTP 服务（axum）：Agent/Workflow/Execution CRUD、SSE 事件流 |

### Agent 定义（YAML）

```yaml
id: researcher
name: 研究员
model:
  provider: anthropic
  name: claude-sonnet-4-20250514
instructions: |
  你是一名研究助理，专注于信息检索和汇总。
skills:
  - id: web_search
    config:
      max_results: 5
constraints:
  - 回答使用中文
memory:
  enabled: true
  max_context_turns: 10
```

### Workflow 引擎

Tavern 使用**事件溯源**架构驱动 workflow：

- 每个 workflow step 产生事件（`WorkflowEvent`）
- 事件持久化到 `EventStore`（SQLite / PostgreSQL）
- 支持重放、恢复、审计
- DAG 调度 + 超时 + 重试 + 信号等待

### Flow 引擎（方法级编排）

```rust
#[derive(Flow)]
struct MyWorkflow {
    // ...
}

#[flow_impl]
impl MyWorkflow {
    #[start]
    async fn research(&self) -> String { /* ... */ }

    #[listen("research_done")]
    async fn summarize(&self) -> String { /* ... */ }

    #[router("summarize_done")]
    async fn decide(&self) -> Vec<String> {
        vec!["publish".into(), "revise".into()]
    }
}
```

### 与 Pandaria 的集成

Tavern **消费** Pandaria 作为底层 Agent 运行时。每个 workflow step 通过 `PandariaRuntime` HTTP adapter 调用 Pandaria 的 Session API。详见 [第五章](./05-integration.md#53-tavern-编排框架)。

---

## 4.5 Constell — LLM 可观测性平台

| 维度 | 说明 |
|------|------|
| 语言 | TypeScript (Node.js 24) |
| 设计参考 | Langfuse |
| 存储 | PostgreSQL (OLTP) + ClickHouse (OLAP) + MinIO/S3 (blob) |
| 当前版本 | v0.3.0-alpha（Tracing 端到端可用） |

### 架构

```
Client SDK ──► web (Next.js) ──► Redis (BullMQ) ──► worker (Express)
                  │                                      │
                  ▼                                      ▼
           PostgreSQL (metadata)                  ClickHouse (events)
                  │                                      │
                  └─────────── MinIO / S3 ───────────────┘
```

### 核心能力

| 能力 | 状态 |
|------|:--:|
| **Tracing** — SDK 发 trace → API → Worker → UI Span 树 | ✅ v0.3.0 |
| **Prompt Management** — 版本化 prompt、UI 编辑器 | 🔜 v0.4.0 |
| **Metrics & Analytics** — 成本、延迟、Token 用量仪表盘 | 🔜 v0.5.0 |
| **Evaluation** — LLM-as-a-Judge、数据集回归 | 📋 v1.0+ |
| **离线 Guard** — 异常行为检测、越权分析、合规审计 | 📋 v1.0+ |

### 设计原则

- **Wide Events First**：observation 是主要分析单元，trace 只是关联标识
- **Immutable Events**：追加型事件记录，避免读时去重
- **Columnar Access**：围绕列式存储设计查询路径（ClickHouse）
- **Scale-Aware APIs**：要求时间窗口、暴露字段选择、token 分页

### 与 Pandaria 生态的关系

Constell 是通用 LLM 可观测性平台，不硬编码 Pandaria 概念。Pandaria 生态项目通过 `ConstellReporter`（独立 crate）将 agent 事件转为 Constell ingestion 格式上报。详见 [第五章](./05-integration.md#54-constell-可观测性平台)。

---

## 4.6 Heirloom — AI 原生语义本体系统（设计期）

| 维度 | 说明 |
|------|------|
| 语言 | 待定（可能 Rust） |
| 设计参考 | Palantir Ontology |
| 阶段 | 设计期——白皮书 + 9 篇 ADR 已完成 |

### 核心理念

Heirloom 解决的是「AI Agent 如何安全地访问企业数据」。它提供一个以 **Resource** 为核心的、具有**类型级安全保证**的本体层——Agent 不需要知道数据在 PostgreSQL 的哪张表，只需要知道业务中有 `Customer`、`Order` 这些概念。

**关键设计决策**：

- **类型级安全**：一个 Resource Type 是否允许被删除——不是 RBAC 配置，是类型定义。`Customer` 类型没有声明 `drop` ability → 没有任何 Role 能创建可删除 Customer 的 Action
- **统一校验链**：人类用户、AI Agent、自动化工作流——三者经过完全相同的 Auth → Role → Capability → Action 九步流水线
- **语义查询 DSL**：JSON DSL 描述跨源查询，Mapping Engine 翻译为底层物理查询

### 实施阶段

| 阶段 | 内容 | 预计周期 |
|------|------|:---:|
| Phase 0 | Schema Registry + Resource Store + 基础 API | 6-8 周 |
| Phase 1 | Semantic Query Layer — Mapping Engine + JSON DSL | 8-10 周 |
| Phase 2 | Secure Operations — Abilities + Action + Role/Capability | 10-12 周 |
| Phase 3 | AI Agent Integration — Agent SDK + Function + 语义搜索 | 8-10 周 |
| Phase 4+ | Governance & Scale | 持续 |

### 与 Pandaria 的集成路径

- **短期**（Phase 1 完成后）：`HeirloomTool`（类似于 HttpProxyTool）让 Agent 以 JSON DSL 查询 Heirloom
- **中期**（Phase 2 完成后）：Heirloom Action 作为 Pawbun 工具的安全后端
- **长期**（Phase 3 完成后）：Agent SDK + Pandaria Agent Loop 深度集成

---

## 4.7 Aspectus — 统一身份与多租户管理

| 维度 | 说明 |
|------|------|
| 语言 | 待定 |
| 阶段 | 规划中（仓库已建：`../Aspectus`） |

### 定位

生态所有项目的单一身份源。统一 `tenant_id`、用户认证、API Key 管理。

### 要解决的问题

当前每个项目各自管理身份——Pandaria 用 HMAC token、Tavern 用 Bearer token、Emerald 用 API Key、Constell 用 NextAuth。同一租户在不同系统中无法关联：「租户 T 的所有活动」无法跨项目查询。

### 目标

- **统一认证**：OAuth2 / OIDC Provider，所有项目通过 token introspection 验证
- **统一租户模型**：`tenant_id` 跨所有项目一致，支持子租户层级
- **API Key 管理**：per-tenant, per-project scoped 的 API Key 签发与轮转
- **跨项目审计**：统一的认证/授权决策日志

### 与其他项目的关系

每个项目在接收请求时调用 Aspectus 的 token introspection 端点，获得统一的 `tenant_id` + `user_id` + `scopes`——项目内部不再各自签发 token。

---

## 4.8 Tokencamp — LLM 网关

| 维度 | 说明 |
|------|------|
| 语言 | 待定 |
| 设计参考 | LiteLLM |
| 阶段 | 规划中（仓库已建：`../tokencamp`） |

### 定位

多 Provider 统一接入层，未来替代 Pandaria 内嵌的 `ai-provider` crate。

### 要解决的问题

当前 `ai-provider` 编译进 agent-core——每个 Pandaria 实例各自管理 LLM 连接和 API Key。Tokencamp 将其抽取为独立网关：

- **API Key 集中管理**：与 Aspectus 协同，租户维度的 Key 和配额统一管理
- **多 Provider 路由**：负载均衡、fallback、A/B 测试
- **Token 计量**：per-tenant、per-model 实时统计
- **热更新**：新增 Provider 无需重启 Pandaria

### 与 Pandaria 的集成

```
当前：Pandaria → ai-provider crate → Provider
未来：Pandaria → Tokencamp HTTP API → Provider
```

过渡期新增 `TokencampProvider`，将 LLM 请求转发到 Tokencamp。

---

## 4.9 依赖方向

```
依赖方向（严格单向，禁止反向依赖）：

                    ┌──────────────────────────────┐
                    │           Aspectus            │
                    │     (被所有项目依赖——认证/租户)    │
                    └──────────────────────────────┘

  Tavern ──► Pandaria ──► Emerald
    │            │
    │            ├──► Pawbun (编译期)
    │            ├──► Tokencamp (LLM 网关，替代 ai-provider)
    │            │
    │            └──► Heirloom (via HeirloomTool)
    │                   (独立项目，仅与 Pandaria 有连线)
    │
    └────────────────┬─────────────────────┘
                     │
                     ▼
                  Constell
              (可观测数据上报)

  Spellpaw --> Pandaria   (应用消费者)
  Daypaw   --> Pandaria   (Agent 工作台)
```

| 关系 | 谁依赖谁 | 方式 | 状态 |
|------|----------|------|:--:|
| Pandaria → Emerald | Pandaria 依赖 Emerald | HTTP (`EmeraldMemoryStore`) | ✅ |
| Pandaria → Pawbun | Pandaria 依赖 Pawbun | Cargo dependency | 📋 |
| Pandaria → Tokencamp | Pandaria 通过 Tokencamp 调用 LLM | HTTP | 💡 |
| Pandaria → Heirloom | Pandaria Agent 通过 HeirloomTool 查询 | HTTP | 📋 |
| Tavern → Pandaria | Tavern 依赖 Pandaria | HTTP (`PandariaRuntime`) | ✅ |
| Emerald/Pandaria/Tavern → Constell | 可观测数据上报 | HTTP (ingestion API) | 📋 |
| 全部 → Aspectus | 所有项目调用 Aspectus 做认证 | HTTP (token introspection) | 💡 |
| Spellpaw → Pandaria | Spellpaw 消费 Pandaria API | HTTP (REST + SSE) | 🔜 Phase 2 |
| Daypaw → Pandaria | Daypaw 消费 Pandaria API | HTTP (REST + SSE) | 🟡 开发中 |

> **Heirloom 和 Constell 无连线。** Heirloom 是独立的数据语义层，不接入 Constell 的可观测管道。

---

## 4.12 技术栈汇总

| 项目 | 语言 | 运行时 | 存储 | 状态 |
|------|------|--------|------|:--:|
| Pandaria | Rust | tokio | PostgreSQL / Redis | ✅ |
| Emerald | Python | FastAPI / async | 图数据库 + 向量存储 + S3 | ✅ |
| Pawbun | Rust | tokio | 无持久化（库） | ✅ |
| Tavern | Rust | tokio / axum | SQLite / PostgreSQL (EventStore) | ✅ |
| Constell | TypeScript | Node.js 24 | PostgreSQL + ClickHouse + Redis + MinIO | 🟡 v0.3 |
| Heirloom | 待定（可能 Rust） | 待定 | PostgreSQL → 图存储 → ... | 📐 设计期 |
| Aspectus | 待定 | 待定 | PostgreSQL | 💡 规划中 |
| Tokencamp | 待定 | 待定 | PostgreSQL / Redis | 💡 规划中 |
| Spellpaw | TypeScript (React 19) | Vite 8 (SPA) | — | 🟡 Phase 1 |
| Daypaw Agent Studio | TypeScript (React 19) | Vite (SPA) | — | 🟡 开发中 |

---

## 4.13 下一步

- 理解具体如何集成 → [第五章：集成指南](./05-integration.md)
- 理解如何部署 → [第六章：部署与运维](./06-deployment.md)
