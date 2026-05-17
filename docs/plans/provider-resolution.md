# Plan: Provider Resolution & Routing Layer 实施计划

> 创建日期: 2026-05-16
> 状态: Draft
> 对应 Spec: `docs/specs/provider-resolution.md`

---

## 1. 实施范围

在 `ai-provider` crate 中新增 Resolver + RouterProvider + OpenAiCompatibleProvider，并在 `agent-core` 中适配 `model_metadata` 调用点。

---

## 2. 任务分解

### Phase 1: ProviderResolver（无外部依赖）

**2.1 修改 `crates/ai-provider/Cargo.toml`**
- [ ] 添加 `dashmap = "6"` 依赖（或 `workspace = true`）

**2.2 新增 `crates/ai-provider/src/resolver.rs`**
- [ ] 定义 `ResolvedModel` 类型（含新增 `api_type` 字段）
- [ ] 定义 `ProviderFactory` 枚举（OpenAi / Anthropic / Google / DeepSeek / Mistral / OpenAiCompatible）
- [ ] 定义 `ProviderRule` struct（含 `factory`、`default_base_url`、`env_key`、`api_type`、`compat_hints`、`fallback_context_window`、`fallback_max_tokens`）
- [ ] 定义 `ProviderResolver` struct，含内置规则表（`LazyLock<HashMap>`）
- [ ] 实现 `resolve(model_spec: &str) -> Result<ResolvedModel, LlmError>`
- [ ] 内置规则表覆盖：openai, anthropic, google, deepseek, mistral, openrouter, ollama
- [ ] 实现 `resolve_openrouter` 特殊规则（提取 underlying provider hint，注入 compat，api_type="anthropic-messages" 或 "openai-completions"）
- [ ] 实现 `resolve_ollama` 特殊规则（`OLLAMA_HOST` env fallback，base_url 含完整路径 `/v1/chat/completions`）
- [ ] 实现 `get_rule(provider_name)` 辅助方法
- [ ] 实现 `default_base_url(provider_name)` 辅助方法
- [ ] 内联单元测试（标准格式、OpenRouter 嵌套、Ollama、DeepSeek、Mistral、未知 provider、边界情况）

**预计改动量**: ~350 行
**预计时间**: 1.5-2 小时

### Phase 2: ProviderConfig 改造 + OpenAiCompatibleProvider

**2.3 修改 `crates/ai-provider/src/providers/shared.rs`**
- [ ] 将 `ProviderConfig.provider_name` 从 `&'static str` 改为 `String`
- [ ] `ProviderConfig::new` 和 `with_client` 签名改为接受 `&str`（移除 `'static` 约束），内部转为 `String`
- [ ] 修改 `define_provider!` 宏：生成的 `provider_name()` 从返回 `$provider_str` 改为 `&self.config.provider_name`
- [ ] 验证所有现有 provider 编译通过

**2.4 新增 `crates/ai-provider/src/providers/openai_compatible.rs`**
- [ ] 定义 `OpenAiCompatibleProvider` struct（直接持有 `ProviderConfig`，支持运行时 provider_name 覆盖）
- [ ] 实现 `LlmProvider`：
  - `provider_name()` 返回 override_name
  - `models()` 返回空列表（RouterProvider 负责聚合）
  - `config()` 返回内部 `ProviderConfig`
  - `stream()` 自行 spawn 任务调用 `openai_compatible_stream(..., &self.override_name, ...)`，复用 macro 生成的 spawning / panic capture 模式
- [ ] 内联单元测试（验证 provider_name 覆盖、stream 中正确的 compat 检测）

**预计改动量**: ~130 行（shared.rs ~±5 行 + 宏修改 ~±2 行 + openai_compatible.rs ~120 行）
**预计时间**: 45 分钟

### Phase 3: LlmProvider trait 扩展

**2.5 修改 `crates/ai-provider/src/provider.rs`**
- [ ] `LlmProvider` trait 新增 `model_metadata(&self, model: &str) -> Option<Model>` 默认实现
- [ ] 添加 `use crate::models::get_model;` import
- [ ] `MockProvider` 测试 impl：因 `model_metadata` 有默认实现，通常无需修改；若新增测试直接调用 `MockProvider::model_metadata()`，则需补充实现返回固定的 `Model`
- [ ] 新增 `model_metadata` 相关单元测试

**2.6 修改 `crates/ai-provider/src/models.rs`**
- [ ] 新增 `get_model_by_spec(spec: &str) -> Option<Model>` 辅助函数（可选，供 resolver 内部使用）

**预计改动量**: ~50 行
**预计时间**: 30 分钟

### Phase 4: RouterProvider（依赖 Phase 1+2+3）

**2.7 新增 `crates/ai-provider/src/router.rs`**
- [ ] 定义 `RouterProvider` struct（含 `ProviderResolver`、`default_config: ProviderConfig`、`DashMap` 缓存）
- [ ] 实现 `LlmProvider` for `RouterProvider`
  - `provider_name()` → `"router"`
  - `config()` → 返回占位 `ProviderConfig`
  - `models()` → 从静态注册表聚合所有已知 provider 模型，格式 `"provider/model_id"`（Ollama 动态模型作为 P2 功能，v1 不实现）
  - `model_metadata()` → resolve 后查注册表；OpenRouter 用 underlying provider 查；都找不到则 fallback
  - `stream()` → resolve → 确定 base_url → `get_or_create_provider` → 合并 options → 调用底层
- [ ] `get_or_create_provider(name, base_url)`：按 `(provider_name, base_url)` 查缓存，无则用 `ProviderFactory` 创建
- [ ] `build_fallback_model(resolved)`：用 `ProviderRule` 中的默认值构建 fallback `Model`
- [ ] 内联单元测试（MockProvider 路由、缓存复用、缓存重建、model_metadata 路由、OpenRouter fallback）

**预计改动量**: ~400 行
**预计时间**: 2-3 小时

### Phase 5: agent-core 适配

**2.8 修改 `crates/agent-core/src/harness/session.rs`**
- [ ] `model_context_window()`：`ai_provider::get_model(provider_name, model)` → `self.provider.model_metadata(&self.model)`

**2.9 修改 `crates/agent-core/src/harness/agent_loop.rs`**
- [ ] image support 检查：`ai_provider::get_model(provider_name, model)` → `self.config.provider.model_metadata(&self.config.model)`
- [ ] `target_api` 获取：从 `provider.model_metadata(&model)` 中提取 `api` 字段（如 `"openai-completions"`、`"anthropic-messages"`）
- [ ] `target_api` fallback：若 `model_metadata()` 返回 `None`，`target_api` 设为 `None`（让 `transform_messages` 使用默认行为），避免传入 `"router"
- [ ] 注意：`transform_messages` 的调用点在 `run_turn()` 中，需确保 `model_metadata()` 在每次 turn 时正确解析（支持 `set_model` 后的切换）

**2.10 新增 agent-core 集成测试**
- [ ] `session::tests::cross_provider_switch`：验证 SessionActor + RouterProvider 跨 provider 切换
- [ ] `session::tests::transform_target_api`：验证 transform_messages 使用正确的 target_api

**预计改动量**: ~40 行 + 测试
**预计时间**: 1-1.5 小时

### Phase 6: 模块导出与编译修复

**2.11 修改 `crates/ai-provider/src/lib.rs`**
- [ ] 导出 `resolver` 和 `router` 模块
- [ ] 导出 `ResolvedModel`、`ProviderResolver`、`RouterProvider`、`ProviderFactory`
- [ ] 导出 `OpenAiCompatibleProvider`

**2.12 修改 `crates/ai-provider/src/providers/mod.rs`**
- [ ] 导出 `openai_compatible` 模块

**2.13 编译验证**
- [ ] `cargo check --all`
- [ ] `cargo test --package ai-provider`
- [ ] `cargo test --package agent-core`

**预计时间**: 30-45 分钟

---

## 3. 文件清单

### 新增文件

| 路径 | 大小预估 | 说明 |
|---|---|---|
| `crates/ai-provider/src/resolver.rs` | ~350 行 | 标识符解析 + 规则表 + ProviderFactory |
| `crates/ai-provider/src/router.rs` | ~400 行 | RouterProvider 实现（含缓存、fallback、base_url 处理） |
| `crates/ai-provider/src/providers/openai_compatible.rs` | ~120 行 | OpenAiCompatibleProvider（直接调用 openai_compatible_stream） |

### 修改文件

| 路径 | 改动量 | 说明 |
|---|---|---|
| `crates/ai-provider/Cargo.toml` | ~+1 行 | 添加 `dashmap` 依赖 |
| `crates/ai-provider/src/providers/shared.rs` | ~±3 行 | `ProviderConfig.provider_name` 改为 `String` |
| `crates/ai-provider/src/provider.rs` | ~+20 行 | trait 新增 `model_metadata` |
| `crates/ai-provider/src/models.rs` | ~+15 行 | 新增 `get_model_by_spec` |
| `crates/ai-provider/src/lib.rs` | ~+8 行 | 导出新模块和类型 |
| `crates/ai-provider/src/providers/mod.rs` | ~+1 行 | 导出新模块 |
| `crates/agent-core/src/harness/session.rs` | ~±3 行 | `model_context_window` 调用点 |
| `crates/agent-core/src/harness/agent_loop.rs` | ~±5 行 | image support + target_api 调用点 |

---

## 4. 风险与回滚策略

| 风险 | 影响 | 缓解 |
|---|---|---|
| `model_metadata` trait 新增破坏下游 crate | 若其他 crate 有自定义 LlmProvider 实现，需同步添加默认实现 | 提供默认实现，已有 impl 自动兼容；编译失败时按错误提示添加即可 |
| RouterProvider 缓存实例泄漏 | 长期运行后缓存膨胀 | 实例仅含 `ProviderConfig`（轻量）， DashMap 本身不限制大小；v1 不实现驱逐，v2 可加 LRU |
| Ollama 未启动时 `stream()` 失败 | Ollama 服务不可用时请求报错 | 返回标准 `LlmError::NetworkError`，由调用方（agent-core）的 retry 逻辑处理 |
| base_url 变化导致缓存未命中 | 频繁切换不同 base_url 的同一 provider 会创建多个实例 | 实例创建成本极低（仅含 Config），不影响性能；长期运行可考虑 LRU 驱逐 |
| OpenAiCompatibleProvider 的 provider_name 未传递到 openai_compatible_stream | inner 固定使用 "openai"，导致 detect_openai_compat / get_model / 事件 metadata 全部错误 | `OpenAiCompatibleProvider` 不委托 `stream()`，而是自行 spawn 任务直接调用 `openai_compatible_stream(..., &self.override_name, ...)`，确保 provider_name 贯穿整个请求生命周期 |

**回滚策略**：RouterProvider 是纯新增组件，不修改任何现有 provider 的核心逻辑。若出现问题，调用方可直接回退到原来的 `Arc<OpenAiProvider>` 等独立 provider 实例，无需回滚代码。

---

## 5. 验收标准

- [ ] `ProviderResolver::resolve("openai/gpt-5.2")` 返回正确 `ResolvedModel`，api_type="openai-completions"
- [ ] `ProviderResolver::resolve("openrouter/anthropic/claude-sonnet-4")` 返回含 Anthropic cache compat、api_type="anthropic-messages" 的 `ResolvedModel`
- [ ] `ProviderResolver::resolve("ollama/llama3.1")` 返回 base_url=`http://localhost:11434/v1/chat/completions` 的 `ResolvedModel`
- [ ] `ProviderResolver::resolve("deepseek/deepseek-chat")` 返回 factory=DeepSeek 的 `ResolvedModel`
- [ ] `ProviderResolver::resolve("mistral/mistral-large")` 返回 factory=Mistral 的 `ResolvedModel`
- [ ] `RouterProvider` 实现 `LlmProvider`，`stream()` 正确路由到不同底层 provider（含 Mistral / DeepSeek 特殊逻辑保留）
- [ ] `RouterProvider.model_metadata("openrouter/anthropic/claude-sonnet-4")` 成功通过 underlying provider 查注册表
- [ ] base_url 改变时缓存正确重建，不复用旧实例
- [ ] `SessionActor::set_model("openai/gpt-5.2")` → `set_model("anthropic/claude-sonnet-4")` 可在同一会话中工作
- [ ] cross-provider 切换时 `transform_messages` 使用正确的 `target_api`
- [ ] 所有现有测试通过（`cargo test --all`）
- [ ] 新增测试覆盖率 ≥ 80%
