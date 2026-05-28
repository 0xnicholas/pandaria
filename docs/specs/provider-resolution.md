# Spec: Provider Resolution & Routing Layer

> 版本: 1.1
> 状态: Completed ✅ — ProviderResolver + built-in rules delivered
> 对应 Plan: `docs/plans/provider-resolution.md`

---

## 1. 术语定义

| 术语 | 定义 |
|---|---|
| **Model Spec** | 用户传入的模型标识符，格式为 `provider/model_id`，如 `openai/gpt-5.2` |
| **RouterProvider** | 实现 `LlmProvider` trait 的统一路由入口，对 `agent-core` 透明 |
| **ProviderResolver** | 将 Model Spec 解析为 `ResolvedModel` 的纯函数组件 |
| **ResolvedModel** | 解析结果，包含目标 provider 名、实际 model_id、base_url、compat 覆盖等 |
| **ProviderRegistry** | `RouterProvider` 内部缓存，管理底层 provider 实例的生命周期 |
| **OpenRouter Nested** | OpenRouter 的模型标识符含多级路径，如 `openrouter/anthropic/claude-sonnet-4` |
| **ProviderFactory** | 创建底层 provider 实例的工厂枚举，区分各 provider 的特殊实现 |

---

## 2. 问题陈述

当前架构中，`SessionActor` 持有固定的 `Arc<dyn LlmProvider>` 和 `model: String`。当用户调用 `set_model("anthropic/claude-sonnet-4")` 时，如果当前 provider 是 `OpenAiProvider`，调用会失败——因为底层 API 格式完全不兼容。

**核心需求**：
- 同一个 `SessionActor` 实例在运行时可自由切换不同 provider 的模型
- 用户只需传入统一标识符（如 `openai/gpt-5.2`），无需关心底层 provider 实例化
- 支持 OpenRouter、Ollama 等特殊 provider 场景
- 对 `agent-core` 完全透明，不破坏现有 `LlmProvider` trait 抽象

---

## 3. 设计目标

| 目标 | 优先级 | 说明 |
|---|---|---|
| 统一标识符路由 | P0 | `provider/model` 格式自动解析并路由到正确底层 provider |
| 运行时跨 provider 切换 | P0 | `set_model()` 可在 OpenAI / Anthropic / Google 之间自由切换 |
| 对 agent-core 透明 | P0 | `SessionActor` 仍只看到一个 `Arc<dyn LlmProvider>` |
| OpenRouter 支持 | P1 | 支持 `openrouter/provider/model` 嵌套格式，自动应用正确 compat |
| Ollama 本地模型支持 | P1 | 支持 `ollama/llama3.1`，自动使用 localhost base_url |
| Ollama 动态发现 | P2 | 运行时拉取 `localhost:11434/api/tags` 获取可用模型列表 |

---

## 4. 架构设计

### 4.1 整体架构

```
┌─────────────────────────────────────────────────────────────────────┐
│                         agent-core                                   │
│  SessionActor                                                        │
│    ├── model: "openai/gpt-5.2" ──► set_model("anthropic/claude-...")│
│    └── provider: Arc<dyn LlmProvider>  ← RouterProvider             │
└─────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────┐
│                      ai-provider                                     │
│                                                                      │
│  ┌──────────────────┐   ┌──────────────────┐   ┌─────────────────┐  │
│  │ RouterProvider   │──►│ ProviderResolver │──►│ ResolvedModel   │  │
│  │ (LlmProvider)    │   │                  │   │                 │  │
│  └──────────────────┘   └──────────────────┘   └─────────────────┘  │
│         │                                                            │
│         ▼                                                            │
│  ┌─────────────────────────────────────────────────────────────────┐│
│  │ ProviderRegistry (DashMap 缓存)                                  ││
│  │                                                                  ││
│  │  key: (provider_name, base_url)                                  ││
│  │                                                                  ││
│  │  ("openai", api.openai.com)      → OpenAiProvider               ││
│  │  ("anthropic", api.anthropic.com)→ AnthropicProvider            ││
│  │  ("deepseek", api.deepseek.com)  → DeepSeekProvider             ││
│  │  ("mistral", api.mistral.ai)     → MistralProvider              ││
│  │  ("google", generativelanguage..)→ GoogleProvider               ││
│  │  ("openrouter", openrouter.ai)   → OpenAiCompatibleProvider     ││
│  │  ("ollama", localhost:11434)     → OpenAiCompatibleProvider     ││
│  └─────────────────────────────────────────────────────────────────┘│
└─────────────────────────────────────────────────────────────────────┘
```

### 4.2 新增数据模型

#### `ResolvedModel`

```rust
pub struct ResolvedModel {
    /// 底层 provider 名称（如 "openai", "anthropic", "openrouter"）
    pub provider_name: String,
    /// 传给底层 provider stream() 的实际 model_id
    pub model_id: String,
    /// 覆盖的 base_url（如 Ollama 的 localhost）
    pub base_url: Option<String>,
    /// 覆盖的 API key
    pub api_key: Option<SecretString>,
    /// 额外 headers
    pub headers: Option<HashMap<String, String>>,
    /// Compat 覆盖（OpenRouter 需要根据底层 provider 注入不同 compat）
    /// v1 中由底层 provider 的 stream() 自行推断，此字段保留用于未来自定义 provider 显式覆盖
    pub compat: Option<ModelCompat>,
    /// 底层 API 协议标识（如 "openai-completions"、"anthropic-messages"），用于 agent-core 的 target_api 和 fallback model 构建
    pub api_type: String,
}
```

#### `ProviderFactory`

```rust
/// 创建底层 provider 实例的工厂。
/// 每个变体对应一个具体的 provider struct，保留其特殊实现逻辑。
pub enum ProviderFactory {
    OpenAi,
    Anthropic,
    Google,
    DeepSeek,
    Mistral,
    /// 用于 OpenRouter / Ollama / 自定义代理等 OpenAI-compatible 端点
    OpenAiCompatible {
        provider_name: String,
        env_key: &'static str,
    },
}
```

#### `ProviderRule`

```rust
pub struct ProviderRule {
    pub factory: ProviderFactory,
    pub default_base_url: String,
    pub env_key: &'static str,
    /// API 协议标识（如 "openai-completions"、"anthropic-messages"），用于 agent-core 的 target_api 和 fallback model 构建
    pub api_type: &'static str,
    pub compat_hints: Option<ModelCompat>,
    /// 当模型不在注册表中时，fallback model 的默认值
    pub fallback_context_window: u32,
    pub fallback_max_tokens: u32,
}

pub struct ProviderResolver {
    /// 内置规则表：provider_name → ProviderRule
    rules: HashMap<String, ProviderRule>,
    /// 用户自定义覆盖
    custom: HashMap<String, ProviderRule>,
}

impl ProviderResolver {
    /// 创建默认解析器，内置所有已知 provider 规则
    pub fn new() -> Self {
        Self {
            rules: Self::build_builtin_rules(),
            custom: HashMap::new(),
        }
    }

    /// 注册自定义 provider 规则（覆盖内置规则）
    pub fn register(&mut self, name: String, rule: ProviderRule) {
        self.custom.insert(name, rule);
    }

    fn build_builtin_rules() -> HashMap<String, ProviderRule> {
        // openai, anthropic, google, deepseek, mistral, openrouter, ollama
        // ...
    }
}
```

### 4.3 解析规则

**标准格式** `provider/model_id`：
1. 按第一个 `/` 拆分，得到 `provider` 和 `model_id`
2. 在规则表中查找 `provider`
3. 若找到，返回 `ResolvedModel { provider_name: provider, model_id, base_url: None, api_key: None, headers: None, api_type: rule.api_type.to_string(), ... }`
4. 若未找到，返回 `LlmError::UnknownProvider`

> **注**：`api_key` 和 `headers` 在 v1 中始终为 `None`。API key 的解析由 `ProviderConfig.resolve_api_key()` 和 `StreamOptions.api_key` 的级联逻辑处理（per-request override → ProviderConfig → env var）。

**OpenRouter 嵌套格式** `openrouter/underlying/model_id`：
1. 检测到 provider 为 `openrouter`
2. 保留剩余全部路径作为 `model_id`（如 `anthropic/claude-sonnet-4`）
3. 提取第二个 segment 作为 `underlying_provider` hint（用于 compat 推断）
4. `base_url` 固定为 `https://openrouter.ai/api/v1/chat/completions`
5. `factory = ProviderFactory::OpenAiCompatible { provider_name: "openrouter", env_key: "OPENROUTER_API_KEY" }`
6. 若 underlying 为 `anthropic`，注入 `cache_control_format: Anthropic`，`api_type = "anthropic-messages"`
7. 否则 `api_type = "openai-completions"`

**Ollama 格式** `ollama/model_id`：
1. `provider_name = "ollama"`
2. `base_url` 优先级：`OLLAMA_HOST` env（若存在且不以 `/v1` 结尾，追加 `/v1/chat/completions`）→ `http://localhost:11434/v1/chat/completions`
3. `factory = ProviderFactory::OpenAiCompatible { provider_name: "ollama", env_key: "OLLAMA_API_KEY" }`
4. `api_type = "openai-completions"`

### 4.4 RouterProvider 实现

```rust
pub struct RouterProvider {
    resolver: ProviderResolver,
    /// 默认 ProviderConfig，用于满足 trait 的 config() 方法
    default_config: ProviderConfig,
    /// (provider_name, base_url) → provider instance
    cache: DashMap<(String, String), Arc<dyn LlmProvider>>,
}

impl RouterProvider {
    pub fn new() -> Self {
        Self {
            resolver: ProviderResolver::new(),
            default_config: ProviderConfig::new(None, "http://router", "router", "ROUTER_API_KEY"),
            cache: DashMap::new(),
        }
    }
}

#[async_trait]
impl LlmProvider for RouterProvider {
    fn provider_name(&self) -> &str { "router" }

    fn config(&self) -> &ProviderConfig {
        // RouterProvider 自身不使用 config()，返回占位值
        &self.default_config
    }

    fn models(&self) -> Vec<String> {
        // 从静态注册表聚合所有已知 provider 的模型，格式为 "provider/model_id"
        // Ollama 动态模型作为 P2 功能，v1 不实现
    }

    fn model_metadata(&self, model: &str) -> Option<Model> {
        let resolved = self.resolver.resolve(model).ok()?;

        // OpenRouter 特殊处理：用 underlying provider 查注册表
        if resolved.provider_name == "openrouter" {
            let mut segments = resolved.model_id.splitn(2, '/');
            let underlying = segments.next()?;
            let actual_model = segments.next()?;
            return get_model(underlying, actual_model)
                .or_else(|| self.build_fallback_model(&resolved));
        }

        get_model(&resolved.provider_name, &resolved.model_id)
            .or_else(|| self.build_fallback_model(&resolved))
    }

    /// 当模型不在静态注册表中时，用 ProviderRule 的默认值构建 fallback Model
    fn build_fallback_model(&self, resolved: &ResolvedModel) -> Option<Model> {
        let rule = self.resolver.get_rule(&resolved.provider_name).ok()?;
        let base_url = resolved.base_url.clone()
            .unwrap_or_else(|| self.resolver.default_base_url(&resolved.provider_name));
        Some(Model {
            id: resolved.model_id.clone(),
            name: resolved.model_id.clone(),
            api: resolved.api_type.clone(),
            provider: resolved.provider_name.clone(),
            base_url,
            reasoning: false,
            input_modalities: vec![Modality::Text],
            cost: TokenCost::default(),
            context_window: rule.fallback_context_window,
            max_tokens: rule.fallback_max_tokens,
            headers: None,
            compat: rule.compat_hints.clone().unwrap_or(ModelCompat::None),
        })
    }

    async fn stream(
        &self,
        model: &str,
        context: LlmContext,
        options: StreamOptions,
        signal: CancellationToken,
    ) -> Result<AssistantMessageEventStream, LlmError> {
        let resolved = self.resolver.resolve(model)?;

        // 确定实际 base_url：resolved 覆盖 > 规则表默认值
        let base_url = resolved.base_url.clone()
            .unwrap_or_else(|| {
                self.resolver.default_base_url(&resolved.provider_name)
            });

        let provider = self.get_or_create_provider(
            &resolved.provider_name,
            &base_url,
        )?;

        // 合并 resolved 中的 overrides 到 options
        let mut opts = options;
        if let Some(key) = resolved.api_key { opts.api_key = Some(key); }
        if let Some(h) = resolved.headers { opts.headers = Some(h); }

        provider.stream(&resolved.model_id, context, opts, signal).await
    }
}
```

#### `get_or_create_provider`

```rust
fn get_or_create_provider(
    &self,
    provider_name: &str,
    base_url: &str,
) -> Result<Arc<dyn LlmProvider>, LlmError> {
    let key = (provider_name.to_string(), base_url.to_string());

    if let Some(entry) = self.cache.get(&key) {
        return Ok(entry.clone());
    }

    let rule = self.resolver.get_rule(provider_name)?;
    let instance = match &rule.factory {
        ProviderFactory::OpenAi => Arc::new(OpenAiProvider::with_base_url(None, base_url)),
        ProviderFactory::Anthropic => Arc::new(AnthropicProvider::with_base_url(None, base_url)),
        ProviderFactory::Google => Arc::new(GoogleProvider::with_base_url(None, base_url)),
        ProviderFactory::DeepSeek => Arc::new(DeepSeekProvider::with_base_url(None, base_url)),
        ProviderFactory::Mistral => Arc::new(MistralProvider::with_base_url(None, base_url)),
        ProviderFactory::OpenAiCompatible { provider_name: name, env_key } => {
            // OpenRouter / Ollama 等：使用 OpenAiCompatibleProvider，
            // 它直接调用 openai_compatible_stream 并传入正确的 provider_name
            Arc::new(OpenAiCompatibleProvider::new(
                None, base_url, name, env_key,
            ))
        }
    };

    self.cache.insert(key, instance.clone());
    Ok(instance)
}
```

**OpenRouter / Ollama 的 provider_name 传递问题**：
`OpenAiProvider` 的 `provider_name()` 宏生成返回固定字符串 `"openai"`。OpenRouter 需要 `provider_name="openrouter"` 以便 `openai_compatible_stream` 中的 `detect_openai_compat("openrouter", ...)` 正确工作。

如果简单 wrap `OpenAiProvider` 并委托 `stream()`，inner 的 spawning 逻辑会使用 `"openai"` 作为 provider_name，导致：
1. `get_model("openai", model)` 查错注册表
2. `detect_openai_compat("openai", ...)` 返回错误的 compat
3. 事件 metadata 中 `provider` 字段显示 `"openai"` 而非 `"openrouter"`

**解决方案**：
1. 修改 `ProviderConfig`，将 `provider_name` 从 `&'static str` 改为 `String`（`env_key` 保持 `&'static str`，因为环境变量名总是编译时已知）。
2. `OpenAiCompatibleProvider` 直接持有 `ProviderConfig`，并在 `stream()` 中自行 spawn 任务调用 `openai_compatible_stream(..., &self.override_name, ...)`：

```rust
pub struct OpenAiCompatibleProvider {
    config: ProviderConfig,
    override_name: String,
}

impl OpenAiCompatibleProvider {
    pub fn new(api_key: Option<SecretString>, base_url: &str, provider_name: &str, env_key: &str) -> Self {
        Self {
            config: ProviderConfig::new(api_key, base_url, provider_name, env_key),
            override_name: provider_name.to_string(),
        }
    }
}

#[async_trait]
impl LlmProvider for OpenAiCompatibleProvider {
    fn provider_name(&self) -> &str { &self.override_name }
    fn models(&self) -> Vec<String> { vec![] } // RouterProvider 负责聚合，底层返回空列表
    fn config(&self) -> &ProviderConfig { &self.config }

    async fn stream(&self, model: &str, ctx: LlmContext, opts: StreamOptions, signal: CancellationToken)
        -> Result<AssistantMessageEventStream, LlmError> {
        // 复用 macro 生成的 spawning / error handling 模式，
        // 但调用 openai_compatible_stream 时传入 override_name
        let config = &self.config;
        let api_key = config.resolve_api_key(&opts)?;
        let (stream, tx) = AssistantMessageEventStream::new(32);
        let client = config.client.clone();
        let model = model.to_string();
        let base_url = config.base_url.clone();
        let provider_name = self.override_name.clone();
        let provider_name_clone = provider_name.clone();

        let handle = tokio::spawn(async move {
            let result = crate::providers::openai::openai_compatible_stream(
                client, base_url, &model, ctx, opts, &tx, api_key, signal, &provider_name_clone,
            ).await;
            if let Err(e) = result {
                // ... error handling same as macro-generated code
            }
        });
        // ... panic capture same as macro-generated code
        Ok(stream)
    }
}
```

### 4.5 LlmProvider trait 扩展

在 `provider.rs` 的 `LlmProvider` trait 中新增方法：

```rust
/// 查询模型元数据。默认实现使用 provider_name + model 查询全局注册表。
/// RouterProvider 重写此方法以支持跨 provider 解析。
fn model_metadata(&self, model: &str) -> Option<Model> {
    get_model(self.provider_name(), model)
}
```

**影响范围**：
- 所有宏生成的 provider（`define_provider!`）自动继承默认实现，无需修改
- `MockProvider` 需要同步更新测试代码
- `agent-core` 中两处 `get_model()` 调用改为 `provider.model_metadata()`
- `transform_messages` 的 `target_api` 需要从 resolved 的 `api_type` 获取

---

## 5. 边界与约束

### 5.1 不变的设计约束

- `ai-provider` 是纯通信层，不感知 tenant 上下文（per ADR-005）
- `agent-core` 不管理 provider 生命周期，只持有 `Arc<dyn LlmProvider>`
- 所有 provider-specific HTTP/SSE 逻辑仍完全封装在 `ai-provider`

### 5.2 新引入的约束

- `RouterProvider` 的缓存 key 为 `(provider_name, base_url)` 组合，不同 base_url 会创建独立实例
- `ProviderResolver` 的解析是纯同步的（无 I/O），确保 `stream()` 调用无额外异步开销
- `RouterProvider::models()` 在 v1 中只返回静态注册表模型；Ollama 动态模型拉取作为 P2 功能，通过后台 task 定期更新缓存
- `config()` 返回的占位 `ProviderConfig` 仅用于满足 trait 契约，不被业务逻辑使用

---

## 6. 兼容性

| 层面 | 影响 | 说明 |
|---|---|---|
| 现有独立 Provider | 无影响 | OpenAiProvider 等保持完整，可继续单独使用 |
| agent-core 业务逻辑 | 无影响 | 仅替换 `get_model()` 调用点，不改逻辑 |
| SessionActor API | 无影响 | `set_model()` 签名不变，只是传入值可以是跨 provider 的 |
| ModelRegistry | 无影响 | 静态注册表继续工作，RouterProvider 的 `model_metadata()` 先查注册表再 fallback |
| StreamOptions | 无影响 | 已包含 `api_key`、`headers` 字段，resolver 通过覆盖这些字段传递配置 |
| transform_messages | 需适配 | agent-core 需用 `provider.model_metadata(model).map(|m| m.api)` 获取 `target_api` |

---

## 7. 测试策略

### 7.1 单元测试（ai-provider）

| 测试 | 内容 |
|---|---|
| `resolver::tests::resolve_standard` | `openai/gpt-5.2` → provider=openai, model=gpt-5.2 |
| `resolver::tests::resolve_openrouter_nested` | `openrouter/anthropic/claude-sonnet-4` → provider=openrouter, model=anthropic/claude-sonnet-4, compat 含 Anthropic cache |
| `resolver::tests::resolve_ollama` | `ollama/llama3.1` → provider=ollama, base_url=localhost:11434/v1/chat/completions |
| `resolver::tests::resolve_unknown_provider` | `unknown/model` → LlmError::UnknownProvider |
| `resolver::tests::resolve_deepseek` | `deepseek/deepseek-chat` → factory=DeepSeek，保留 provider_name="deepseek" |
| `resolver::tests::resolve_mistral` | `mistral/mistral-large` → factory=Mistral，保留 provider_name="mistral" |
| `router::tests::route_to_openai` | RouterProvider.stream("openai/gpt-5.2", ...) 路由到 OpenAiProvider |
| `router::tests::route_to_anthropic` | RouterProvider.stream("anthropic/claude-sonnet-4", ...) 路由到 AnthropicProvider |
| `router::tests::route_to_mistral` | RouterProvider.stream("mistral/mistral-large", ...) 路由到 MistralProvider，保留 reasoning 格式 |
| `router::tests::route_to_openrouter` | RouterProvider.stream("openrouter/anthropic/claude-sonnet-4", ...) 路由到 OpenAiCompatibleProvider，provider_name="openrouter" |
| `router::tests::model_metadata_routing` | RouterProvider.model_metadata("openai/gpt-5.2") 返回正确 Model |
| `router::tests::model_metadata_openrouter` | RouterProvider.model_metadata("openrouter/anthropic/claude-sonnet-4") 用 anthropic provider 查注册表 |
| `router::tests::cache_reuse` | 两次 resolve 到同一 (provider_name, base_url) 复用同一实例 |
| `router::tests::cache_rebuild_on_base_url_change` | base_url 改变时创建新实例，不复用旧实例 |
| `openai_compatible::tests::provider_name_override` | `OpenAiCompatibleProvider::provider_name()` 返回正确的 override 值 |
| `openai_compatible::tests::stream_uses_override_name` | stream 中产生的 `AssistantMessage` 的 `provider` 字段使用 override_name |

### 7.2 集成测试（agent-core）

| 测试 | 内容 |
|---|---|
| `session::tests::cross_provider_switch` | 创建 RouterProvider → SessionActor → prompt("openai/gpt-5.2") → set_model("anthropic/claude-sonnet-4") → prompt() → 验证两次请求到达不同 MockProvider |
| `session::tests::transform_target_api` | 验证 cross-provider 时 transform_messages 使用正确的 target_api（"openai-completions" vs "anthropic-messages"） |
