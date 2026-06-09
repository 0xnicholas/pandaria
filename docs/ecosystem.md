# Pandaria 生态 / 卫星项目概览

> **Pandaria** 是面向服务端多租户的 agent runtime & harness（Rust 实现）。  
> 以下十个项目（含规划中）构成完整生态。八个基础设施项目 + 两个应用层项目。

---

## 生态全景图

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

        ┌──────────────────────────────┐
        │   Spellpaw · Daypaw Studio   │
        │     应用层（Pandaria 消费者）   │
        └──────────────┬───────────────┘
                       │ HTTP + SSE
                       ▼
                  ┌──────────┐
                  │ Pandaria │
                  └──────────┘
```

---

## 项目清单

| 项目 | 维度 | 状态 | 一句话 |
|------|------|:--:|------|
| **Pandaria** | Agent Runtime + 在线 Guard | ✅ 核心 | 多租户 agent 执行引擎——loop、session、hook、在线安全护栏 |
| **Emerald** | 记忆 | ✅ 可用 | 知识图谱记忆系统——不遗忘、有时序、有个性化 |
| **Pawbun** | 工具 | ✅ 可用 | Agent 工具箱——工具注册、MCP 协议、多模态文件 |
| **Tavern** | 编排 | ✅ 可用 | 多 Agent 编排——YAML 定义、DAG 工作流、事件溯源 |
| **Constell** | 可观测 + 评测 + 离线 Guard | 🟡 v0.3 | 追踪、指标、Prompt 管理、LLM 评测、离线安全检测 |
| **Heirloom** | 数据语义 | 📐 设计期 | AI 原生语义本体系统——类型级安全、Action 九步校验 |
| **Aspectus** | 身份与多租户 | 💡 规划中 | 统一身份、认证、租户管理——生态所有项目的单一身份源 |
| **Tokencamp** | LLM 网关 | 💡 规划中 | 多 Provider 统一接入、API Key 管理、token 计量——未来替代 ai-provider |
| **Spellpaw** | 应用（短剧创作） | 🟡 Phase 1 | AI 辅助短剧/短视频创作工具——Pandaria 生态的应用消费者 |
| **Daypaw Agent Studio** | 应用（Agent 构建） | 🟡 开发中 | Agent 可视化构建与配置工作台——Pandaria 的管理界面 |

---

## 1. Pandaria（核心）— Agent Runtime + 在线 Guard

**定位**：面向服务端多租户的 agent runtime & harness。Hook 系统同时承载在线安全护栏（注入检测、内容审查、工具参数校验）——在每次 tool call 和 LLM 响应的热路径上同步执行。

| 维度 | 说明 |
|------|------|
| 语言 | Rust + tokio |
| 核心能力 | Agent Loop（原生 Tool Use 协议）、Session 生命周期、Hook 策略系统、Compaction、Prompt 构建 |
| 多租户 | tokio task 级隔离、资源配额、per-tenant 计量 |
| 持久化 | PostgreSQL / Redis adapter，支持 auto-restore + 增量保存 |
| API | REST + SSE（通过 api-gateway） |
| LLM 接入 | 当前：`ai-provider` crate（Anthropic/OpenAI/Google/Mistral/DeepSeek + Bedrock）；未来：迁移至 Tokencamp 网关 |

**生态中的角色**：一切的中心。自身不提供工具实现、不提供记忆引擎、不提供多 Agent 编排——这些由卫星项目负责。在线 guard 策略作为 hook 模块内置于 DefaultHookDispatcher。

---

## 2. Emerald — 记忆与上下文基础设施

**定位**：面向 AI Agent 的记忆系统，解决「Agent 在每次对话之间会遗忘一切」的问题。

| 维度 | 说明 |
|------|------|
| 语言 | Python |
| 核心模型 | **知识图谱**（非向量数据库）——事实通过关系（更新 / 扩展 / 推导）彼此连接 |
| 关键能力 | 自动提取事实、时序追踪、矛盾解决、智能遗忘、用户画像 |
| 与 RAG 区别 | RAG 回答「我知道什么」，记忆回答「我记得你的什么」——有状态、有时序、有个性化 |
| API 面 | `add` / `search` / `profile` / `upload` 四个核心方法 |

### 与 Pandaria 的集成

Pandaria 通过 **`EmeraldMemoryStore`**（`agent-core/src/memory/emerald.rs`）HTTP adapter 连接 Emerald。`tenant_id` 映射为 Emerald 的 `entity_id`。

---

## 3. Pawbun — Agent 工具 & 多模态库

**定位**：Pandaria 生态的 Rust 工具库。Agent 可用的工具注册/执行框架、MCP 协议、多模态文件处理。

| Crate | 用途 |
|-------|------|
| `pawbun-toolkit` | `Tool` / `AsyncTool` trait、`ToolKit` 注册中心、内置工具、MCP 客户端 |
| `pawbun-toolkit-macros` | `#[pawbun_tool]` 过程宏 |
| `pawbun-files` | 多模态文件处理——加载 + 多 Provider 格式化 + 安全约束 |
| `pawbun-mcp-core` | MCP 协议核心类型（JSON-RPC 2.0、Transport trait） |
| `pawbun-mcp-server` | MCP 服务器（stdio / SSE） |

**对标参考**：CrewAI Tools + CrewAI Files，Rust 实现。

---

## 4. Tavern — 多 Agent 编排框架

**定位**：事件溯源驱动的多 Agent 自动化编排，以 Pandaria 为底层运行时的编排层。

| Crate | 用途 |
|-------|------|
| `tavern-core` | 共享类型：`AgentConfig`、`Runtime` trait |
| `tavern-adapters` | Pandaria HTTP adapter + Mock adapter |
| `tavern-hero` | Agent 注册表、YAML 配置加载 |
| `tavern-comp` | 事件溯源 Workflow 引擎——DAG 调度、重试/信号/超时 |
| `tavern-flow` | 方法级事件驱动编排（`#[start]` / `#[listen]` / `#[router]`） |
| `tavern-server` | HTTP 服务（axum） |

**对标参考**：LangGraph 编排能力，Rust + 事件溯源 + YAML 声明式。

### 与 Pandaria 的集成

Tavern 消费 Pandaria 作为 Agent 执行运行时。每个 workflow step 通过 `PandariaRuntime` HTTP adapter 调用 Session API。

---

## 5. Constell — LLM 可观测性 & 评测 & 离线 Guard 平台

**定位**：轻量级开源 LLM 可观测性平台。追踪、监控、评测、Prompt 管理。同时承载离线安全检测——消费 trace 数据，检测 jailbreak / hallucination / PII 泄露趋势并触发告警。架构对齐 Langfuse。

| 维度 | 说明 |
|------|------|
| 语言 | TypeScript (Node.js 24) |
| 架构 | Next.js (web) + Express (worker) + BullMQ (Redis) |
| 存储 | PostgreSQL (OLTP) + ClickHouse (OLAP) + MinIO/S3 (blob) |
| 当前版本 | v0.3.0-alpha（Tracing 端到端） |

**核心能力**：
- **Tracing** ✅ — SDK → Ingestion → Worker → UI Span 树
- **Prompt Management** 🔜 v0.4.0 — 版本化 prompt、UI 编辑器
- **Metrics & Analytics** 🔜 v0.5.0 — 成本、延迟、Token 仪表盘
- **Evaluation** 📋 v1.0+ — LLM-as-a-Judge、数据集回归、工具调用正确性验证
- **离线 Guard 检测** 📋 — 消费 trace 数据：异常行为检测、越权趋势分析、合规审计回溯

**设计原则**：通用 LLM 可观测性平台，不硬编码 Pandaria 概念。评测和离线 guard 检测都基于其 tracing 管道——先有 trace，再有评分和检测。

---

## 6. Heirloom — AI 原生语义本体系统（设计期）

**定位**：当 AI Agent 进入企业运营，它需要一个**结构化的、可被安全操作的业务语义界面**。Heirloom 提供这个界面——不是数据库直连，不是脆弱的 prompt 约束，而是一个以 Resource 为核心的、具有类型级安全保证的本体层。

| 维度 | 说明 |
|------|------|
| 语言 | 待定（可能 Rust） |
| 阶段 | 设计期——白皮书 + 9 篇 ADR 已完成，Phase 0（Schema Registry + Resource Store）待启动 |
| 核心概念 | Resource、Abilities、Action（九步校验流水线）、Role + Capability、三种关系语义（Ownership / Reference / Association）、状态机 |
| 设计参考 | Palantir Ontology（但关键设计上存在分歧：类型级安全 vs RBAC 配置安全） |

### 核心设计原则

1. **语义层，非数据目录**：Agent 不需要知道数据在 PostgreSQL 的哪张表——只需要知道 `Customer`、`Order` 这些业务概念。Mapping Engine 将语义查询翻译为底层物理查询。
2. **类型级安全**：一个 Resource Type 是否允许被删除——不是 RBAC 配置，是类型定义本身。如果 `Customer` 类型没有声明 `drop` ability，没有任何 Role 能创建可删除 Customer 的 Action。
3. **统一校验链**：人类用户、AI Agent、自动化工作流——三者经过完全相同的 Auth → Role → Capability → Action 流水线。Agent 和人类在同一个安全模型中被平等对待。

### 实施阶段

| 阶段 | 内容 | 预计周期 |
|------|------|:---:|
| Phase 0 | Schema Registry + Resource Store + 基础 API | 6-8 周 |
| Phase 1 | Semantic Query Layer（只读）— Mapping Engine + JSON DSL + Perspective Engine | 8-10 周 |
| Phase 2 | Secure Operations — Abilities + Action 引擎 + Role/Capability + State Machine | 10-12 周 |
| Phase 3 | AI Agent Integration — Agent SDK + Function 引擎 + 语义搜索 | 8-10 周 |
| Phase 4 | Governance & Scale — Ontology 分支、条件式 Abilities、水平扩展 | 12-16 周 |

### 与 Pandaria 生态的集成路径

Heirloom 解决的是「Agent 如何安全访问企业结构化数据」。在 Pandaria 生态中：

- **短期**（Phase 1 完成后）：通过 `HeirloomTool`（类似于 `HttpProxyTool`）让 Agent 以 JSON DSL 查询 Heirloom 的语义层
- **中期**（Phase 2 完成后）：Heirloom 的 Action 引擎可作为 Pawbun 工具的安全后端——工具的 write 操作经 Heirloom Action 校验
- **长期**（Phase 3 完成后）：Heirloom Agent SDK + Pandaria Agent Loop 深度集成——Agent 通过语义层理解业务、通过 Action 操作数据、通过 Emerald 记忆用户上下文

> Heirloom 目前处于设计期（白皮书 + ADR），尚未有可部署代码。以上集成路径为方向性规划。

---

## 7. Aspectus — 统一身份与多租户管理

**定位**：生态所有项目的单一身份源。统一 `tenant_id`、用户认证、API Key 管理、租户配额配置。

| 维度 | 说明 |
|------|------|
| 语言 | 待定 |
| 阶段 | 规划中（仓库已建：`../Aspectus`） |

### 要解决的问题

当前每个项目各自管理身份：

| 项目 | 认证方式 | tenant_id 来源 |
|------|---------|---------------|
| Pandaria | HMAC-SHA256 Bearer token | Token payload 自包含 |
| Tavern | Bearer token + refresh | `PANDARIA_TENANT_ID` 环境变量 |
| Emerald | API Key | `entity_id`（无强校验） |
| Constell | NextAuth + API Key | 项目级隔离 |
| Heirloom（规划） | Auth → Role → Capability | Schema 内嵌 |

同一租户在不同系统中无法关联——Pandaria 的 session、Emerald 的记忆、Tavern 的 workflow execution、Constell 的 trace 各自有各自的 tenant 概念，无法做到「查一下租户 T 在所有系统中的活动」。

### 目标架构

```
Aspectus
  │
  ├── 租户管理 ──► 创建/配置/删除租户 · 配额设置 · 子租户层级
  │
  ├── 认证服务 ──► OAuth2 / OIDC Provider
  │     · 用户登录（human + service account）
  │     · API Key 签发与轮转（per-tenant, per-project scoped）
  │     · Token 自省端点（所有项目验证 token 时调用）
  │
  ├── 授权服务 ──► RBAC / ABAC
  │     · 角色定义（admin / developer / agent / viewer）
  │     · 跨项目权限映射（Pandaria session:create ↔ Tavern workflow:run）
  │
  └── 审计日志 ──► 认证/授权决策的统一审计追踪
```

### 与其他项目的关系

```
                    ┌─────────────┐
                    │  Aspectus   │
                    └──────┬──────┘
                           │ Token 自省 / 租户解析
          ┌────────────────┼────────────────┐
          ▼                ▼                ▼
    ┌──────────┐    ┌──────────┐    ┌──────────┐
    │ Pandaria │    │  Tavern  │    │ 其他项目  │
    │ api-gw   │    │  server  │    │          │
    └──────────┘    └──────────┘    └──────────┘
```

每个项目在接收请求时调用 Aspectus 的 token introspection 端点，验证 token 有效性并获得统一的 `tenant_id` + `user_id` + `scopes`。项目内部不再各自签发 token。

### 与 Heirloom 的角色模型边界

Aspectus 和 Heirloom Phase 2 都有角色/权限概念，粒度不同，须在设计期划清：

| 层 | 谁管 | 粒度 | 例子 |
|----|------|------|------|
| **身份认证** | Aspectus | 「你是谁」 | 用户 U 属于租户 T，持有 API Key K |
| **项目访问** | Aspectus | 「你能进哪个系统」 | 用户 U 可以访问 Pandaria session、Tavern workflow |
| **数据操作** | Heirloom | 「你能对这个 Resource 做什么」 | 用户 U 可以从 Customer 表 read，但不能 drop |

Aspectus = 进门（authentication + coarse-grained access），Heirloom = 进房间后能碰哪些东西（fine-grained data authorization）。两者不重叠。

---

## 8. Tokencamp — LLM 网关

**定位**：多 Provider 统一接入层，未来替代 Pandaria 内嵌的 `ai-provider` crate。对标 LiteLLM，但作为生态内独立服务运行。

| 维度 | 说明 |
|------|------|
| 语言 | 待定 |
| 设计参考 | LiteLLM |
| 阶段 | 规划中（仓库已建：`../tokencamp`） |

### 要解决的问题

当前 `ai-provider` 是 Pandaria 的内嵌 crate——编译进 agent-core，每个 Pandaria 实例各自管理 LLM 连接、API Key、重试逻辑。这带来几个问题：

1. **API Key 分散**：每个 Pandaria 实例需要配置所有 Provider 的 API Key，密钥管理负担随实例数线性增长
2. **无集中计量**：token 消耗在 agent-core 层面统计，无法跨实例汇总
3. **Provider 变更需重启**：新增 Provider 或切换模型需要重新部署 Pandaria
4. **无负载均衡**：无法在多个同类型 Provider 间做 fallback 或负载分发

### 目标

- **统一接入**：Pandaria 只需知道 Tokencamp 的地址，不再直接对接各个 LLM Provider
- **API Key 集中管理**：与 Aspectus 协同——租户维度的 API Key 和配额在 Tokencamp 层管理
- **多 Provider 路由**：负载均衡、fallback、A/B 测试（同一请求试两个模型比较结果）
- **Token 计量**：per-tenant、per-model 的实时 token 消耗统计
- **热更新**：新增 Provider、切换模型、调整路由规则无需重启下游服务

### 与 Pandaria 的集成

```
当前：Pandaria (agent-core) → ai-provider crate → Anthropic/OpenAI/Google/...
未来：Pandaria (agent-core) → Tokencamp HTTP API → Anthropic/OpenAI/Google/...
```

过渡期内 `ai-provider` crate 保留，但新增 `TokencampProvider` 实现——将 LLM 请求转发到 Tokencamp 而非直连 Provider。迁移完成后 `ai-provider` 可逐步废弃。

### 与 Aspectus 的关系

Tokencamp 的租户级 API Key 管理和配额计量依赖 Aspectus 的租户模型。Aspectus 提供「租户 T 的 LLM API Key 是什么、月配额是多少」，Tokencamp 执行路由和计量。

---

## 9. Spellpaw — AI 辅助短剧创作工具（应用层）

**定位**：Pandaria 生态的应用消费者——面向短剧/短视频创作者的 AI 辅助制作工具。不是基础设施，而是站在 Pandaria 肩膀上的端到端产品。

| 维度 | 说明 |
|------|------|
| 语言 | TypeScript (React 19) |
| 技术栈 | Vite 8 · Zustand 5 · Tailwind CSS 4 · React Flow (@xyflow) · dnd-kit |
| 测试 | 44 tests passing（Phase 1） |
| 阶段 | Phase 1 ✅（本地内容编辑）→ Phase 2 🔜（Pandaria AI 集成） |

### 核心理念

- **结构即内容** — 幕→场景→镜头的树状叙事结构是创作骨架，AI Agent 可直接操作
- **对话即操作** — AI Agent 不仅是聊天对象，可通过对话修改项目结构
- **画布即工作台** — 无限画布上的节点卡片是思维的可视化延伸

### 与 Pandaria 的集成（Phase 2 规划）

```
Spellpaw (React SPA)
  │
  ├── 用户编辑项目结构（树 + 画布）
  │
  ├── 用户发起 AI 对话
  │     └── POST /api/v1/sessions  →  Pandaria
  │           │
  │           └── Agent Loop（使用 Pawbun 工具操作项目文件）
  │                 │
  │                 └── Tool: spellpaw_update_scene / spellpaw_add_character / ...
  │                       │
  │                       └── 通过 Spellpaw Tool Server 修改项目状态
  │
  └── SSE 订阅 session 事件 → 实时更新 UI
```

Spellpaw 通过 Tool Server 暴露项目操作能力给 Pandaria Agent——Agent 可以像人类用户一样创建场景、添加角色、调整叙事结构。这种模式下，Spellpaw 既是 Pandaria 的消费者，也是 Pandaria 工具（通过 Pawbun MCP 或 HttpProxyTool 暴露）的提供者。

### 生态定位

Spellpaw 与其余八个项目的本质区别：

| | 八个基础设施项目 | Spellpaw |
|---|---|---|
| 层次 | 基础设施 / 平台层 | 应用层 |
| 用户 | 开发者 / 运维 / 其他项目 | 短剧创作者（终端用户） |
| 与 Pandaria 的关系 | 被依赖（提供能力）或消费（使用能力） | 纯消费 + 提供领域工具 |
| 部署 | 各自独立服务 | SPA + Tool Server |

Spellpaw 验证了 Pandaria 生态的最终价值：**不是建了基础设施就完了——基础设施之上能长出真正有用的产品。**

---

## 10. Daypaw Agent Studio — Agent 可视化构建工作台（应用层）

**定位**：Pandaria 生态的管理界面——Agent 的可视化构建、配置与调试工作台。如果说 Pandaria 是引擎，Daypaw 就是驾驶舱。

| 维度 | 说明 |
|------|------|
| 语言 | TypeScript (React 19) |
| 技术栈 | Vite · MUI 9 · Monaco Editor · React Flow (@xyflow) · js-yaml |
| 模块 | `/studio`（Agent 构建画布）、`/works`（工作空间管理） |
| 阶段 | 🟡 开发中 |

### 核心理念

- **可视化 Agent 编排** — 通过 React Flow 画布拖拽构建 Agent 拓扑（工具链、记忆、hook 策略）
- **YAML 即配置** — Agent 定义与 Tavern 的 YAML 配置打通，Studio 中编辑、Tavern 中执行
- **Monaco 编辑器** — 提供代码级 prompt 编辑和 JSON/YAML schema 校验
- **Pandaria 原生集成** — 直接消费 Pandaria API，支持 session 创建、实时对话、事件流订阅

### 与 Pandaria 生态的关系

```
Daypaw Agent Studio (React SPA)
  │
  ├── /studio ──► 拖拽构建 Agent 拓扑
  │     │           生成 YAML 配置 → Tavern / Pandaria
  │     │
  │     └── Monaco Editor: prompt 编辑 + schema 校验
  │
  ├── /works ──► 管理 Agent 项目 · 工作空间 · 文件
  │
  └── Pandaria API ──► 创建 session · 发送消息 · SSE 事件流
```

Daypaw 是 Pandaria + Tavern 的「前端」——让开发者和运营人员通过 GUI 而非直接写 YAML/curl 来管理 Agent。它与 Spellpaw 的区别：Spellpaw 是面向创作者的垂直应用，Daypaw 是面向开发者的通用 Agent 工作台。

---

## 项目间依赖关系

```
依赖方向（严格单向）：

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

  Spellpaw ──► Pandaria   (应用消费者)
  Daypaw   ──► Pandaria   (Agent 工作台)
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

> **Heirloom 和 Constell 无连线。** Heirloom 是独立的数据语义层，不接入 Constell 的可观测管道，其运维监控独立管理。

---

## 技术栈总览

| 项目 | 语言 | 运行时 | 存储 | 状态 |
|------|------|--------|------|:--:|
| Pandaria | Rust | tokio | PostgreSQL / Redis | ✅ |
| Emerald | Python | FastAPI / async | 图数据库 + 向量存储 + S3 | ✅ |
| Pawbun | Rust | tokio | 无持久化（库） | ✅ |
| Tavern | Rust | tokio / axum | SQLite / PostgreSQL (EventStore) | ✅ |
| Constell | TypeScript | Node.js 24 | PostgreSQL + ClickHouse + Redis + MinIO | 🟡 v0.3 |
| Heirloom | 待定（可能 Rust） | 待定 | PostgreSQL JSONB → 独立图存储 → ... | 📐 设计期 |
| Aspectus | 待定 | 待定 | PostgreSQL | 💡 规划中 |
| Tokencamp | 待定 | 待定 | PostgreSQL / Redis | 💡 规划中 |
| Spellpaw | TypeScript (React 19) | Vite 8 (SPA) | — | 🟡 Phase 1 |
| Daypaw Agent Studio | TypeScript (React 19) | Vite (SPA) | — | 🟡 开发中 |

---

## 快速导航

| 想了解... | 看这里 |
|-----------|--------|
| **📖 生态 Cookbook** | **[`cookbook/`](cookbook/README.md)** — 架构、集成、部署完整指南 |
| Pandaria 核心架构 | [`../AGENTS.md`](../AGENTS.md) |
| Emerald 工作原理 | `../../Emerald/README.md` |
| Pawbun 工具集 | `../../Pawbun/README.md` |
| Tavern 编排引擎 | `../../Tavern/README.md` |
| Constell 可观测性 | `../../Constell/AGENTS.md` |
| Heirloom 语义本体 | `../../Heirloom/AGENTS.md` |
| Emerald 集成 Spec | `specs/2026-05-27-pandaria-emerald-memorystore.md` |
| 生态集成 Spec | `specs/2026-05-28-ecosystem-integration-deepening.md` |

---

*本文档随生态演进持续更新。新增卫星项目或重大集成变更时，同步更新本文档。*
