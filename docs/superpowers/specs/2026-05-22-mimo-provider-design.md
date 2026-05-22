# Mimo Provider 接入设计

> 日期: 2026-05-22
> 状态: 设计中
> 目标: 为 ai-provider 新增小米 Mimo (https://platform.xiaomimimo.com) LLM 支持

---

## 背景

小米 Mimo 开放平台提供 OpenAI 兼容的 Chat Completions API，支持 `mimo-v2.5-pro` 和 `mimo-v2.5` 两个模型。用户希望 pandaria 项目通过 `ai-provider` crate 接入 Mimo，以 `mimo/model-id` 格式使用。

## Mimo API 关键参数

| 参数 | 值 |
|---|---|
| 协议 | OpenAI Chat Completions 兼容 |
| 端点 | `https://api.xiaomimimo.com/v1/chat/completions` |
| 认证头 | `api-key: {key}`（非 `Authorization: Bearer`） |
| 环境变量 | `MIMO_API_KEY` |
| Thinking | 支持 reasoning_content（标准 OpenAI thinking 格式） |
| streaming | 支持 SSE |

### 接入模型

| 模型 ID | 名称 | Context Window | 输入价 ($/1M) | 输出价 ($/1M) |
|---|---|---|---|---|
| `mimo-v2.5-pro` | MiMo V2.5 Pro | 1,048,576 | 0.20 | 1.00 |
| `mimo-v2.5` | MiMo V2.5 | 1,048,576 | 0.08 | 0.40 |

> 价格取海外美元定价。缓存读写当前不计。

---

## 核心设计挑战：认证头差异 & compat 传播路径

### 认证头差异

Mimo 使用 `api-key: {key}` 认证头，而非 OpenAI 标准的 `Authorization: Bearer {key}`。现有的 `openai_compatible_stream` 函数硬编码了 `Authorization: Bearer` 格式，无法直接复用。

**解决方案**: 在 `OpenAiCompat` 中新增 `auth_header` 字段，通过 `detect_openai_compat()` 自动检测，provider 覆盖默认认证头。

### compat 传播路径限制

`openai_compatible_stream` 内部的 `compat` 变量来源于 `get_model(provider_name, model).compat`，而模型注册宏 `insert!` 将 `compat` 固定为 `None`。`ProviderRule.compat_hints`（在 resolver 中定义）**不会**传播到 `openai_compatible_stream` 内部。

因此 `auth_header` 的提供路径为：

```
detect_openai_compat(provider_name, base_url, model_id)
  → OpenAiCompat { auth_header: Some("api-key"), ... }
  → openai_compatible_stream 内部调用 detect_openai_compat 获取 auth_header
```

而非通过 `ProviderRule.compat_hints` 或模型注册的 `compat` 字段。

---

## 设计

### 1. `OpenAiCompat` 扩展 (`compat.rs`)

```rust
pub struct OpenAiCompat {
    // ... 现有字段 ...
    /// 覆盖默认认证头。
    /// Some("api-key") → 请求带 `api-key: {key}`（无 Bearer 前缀）
    /// None             → 保持默认 `Authorization: Bearer {key}`
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub auth_header: Option<String>,
}
```

**影响范围**：
- `detect_openai_compat()` — Mimo provider 时返回 `auth_header: Some("api-key")`
- `merge_openai_compat()` — 新增一行 merge 逻辑
- 现有所有 provider 不受影响（`None` 时行为不变）

#### `detect_openai_compat` Mimo 检测

```rust
// 在 detect_openai_compat 函数末尾的 return 之前
let is_mimo = provider == "mimo" || base_url.contains("xiaomimimo.com");

OpenAiCompat {
    // ... 现有字段 ...
    auth_header: if is_mimo { Some("api-key".to_string()) } else { None },
}
```

### 2. `openai_compatible_stream` 认证逻辑 (`openai.rs`)

**关键**：不能使用模型 `compat`（始终为 `None`），应调用 `detect_openai_compat` 专门获取 `auth_header`。

**变更前**（硬编码）:
```rust
builder.header("Authorization", format!("Bearer {}", api_key.expose_secret()));
```

**变更后**:
```rust
let auto_compat = detect_openai_compat(provider_name, &base_url, model);
let (auth_key, auth_value) = match auto_compat.auth_header.as_ref() {
    Some(header_name) => (header_name.as_str(), api_key.expose_secret().to_string()),
    None => ("Authorization", format!("Bearer {}", api_key.expose_secret())),
};
builder.header(auth_key, auth_value);
```

> 注意：`compat` 变量（用于 thinking/reasoning 逻辑）保持从 `get_model().compat` 获取，不受此改动影响。

### 3. Provider 注册 (`resolver.rs`)

在 `build_builtin_rules()` 中新增。注意 `auth_header` 不在此设置（它走 `detect_openai_compat` 路径），`compat_hints` 仅设置非标准字段：

```rust
rules.insert("mimo".to_string(), ProviderRule {
    factory: ProviderFactory::OpenAiCompatible {
        provider_name: "mimo".to_string(),
        env_key: "MIMO_API_KEY",
    },
    default_base_url: "https://api.xiaomimimo.com/v1/chat/completions".to_string(),
    env_key: "MIMO_API_KEY",
    api_type: "openai-completions",
    compat_hints: None,
    fallback_context_window: 1_048_576,
    fallback_max_tokens: 128_000,
});
```

### 4. 模型元数据 (`models_data.rs`)

```rust
// ── Mimo ──
insert!(m, "mimo", "mimo-v2.5-pro", "MiMo V2.5 Pro",
    "openai-completions",
    "https://api.xiaomimimo.com/v1/chat/completions",
    true,                                     // reasoning
    vec![Modality::Text],
    TokenCost { input: 0.20, output: 1.00, cache_read: 0.0, cache_write: 0.0 },
    1_048_576, 128_000);

insert!(m, "mimo", "mimo-v2.5", "MiMo V2.5",
    "openai-completions",
    "https://api.xiaomimimo.com/v1/chat/completions",
    true,                                     // reasoning
    vec![Modality::Text],
    TokenCost { input: 0.08, output: 0.40, cache_read: 0.0, cache_write: 0.0 },
    1_048_576, 128_000);
```

---

## 文件改动清单

| 文件 | 改动类型 | 说明 |
|---|---|---|
| `crates/ai-provider/src/compat.rs` | 修改 | `OpenAiCompat` 新增 `auth_header` 字段；`detect_openai_compat` 添加 Mimo 分支（`provider == "mimo"` → `auth_header: Some("api-key")`）；`merge_openai_compat` 添加 merge |
| `crates/ai-provider/src/providers/openai.rs` | 修改 | `openai_compatible_stream` 调用 `detect_openai_compat` 获取 `auth_header` 选择认证头 |
| `crates/ai-provider/src/resolver.rs` | 修改 | `build_builtin_rules` 注册 `mimo` |
| `crates/ai-provider/src/models_data.rs` | 修改 | 注册 `mimo-v2.5-pro`、`mimo-v2.5` |

**不涉及新文件**。Mimo 复用 `OpenAiCompatibleProvider` + `openai_compatible_stream`，无需独立 provider 文件。

---

## 风险与约束

- **API 稳定性**: Mimo 目前处于公测阶段，API 可能变化
- **Thinking 行为**: v2.5-pro/v2.5 思考模式下 `temperature` 参数被服务端强制覆盖为 1.0（不在客户端处理，这是 Mimo 服务端行为）
- **认证**: `api-key` 头的值直接暴露 API key（无 Bearer 前缀），与 Mimo 文档一致

---

## 测试计划

1. **单元测试**: `compat.rs` 新增 `detect_openai_compat("mimo", "https://api.xiaomimimo.com/v1/chat/completions", "mimo-v2.5-pro")` 测试，验证 `auth_header = Some("api-key")`
2. **单元测试**: `resolver.rs` 新增 `test_resolve_mimo` 测试，验证 `mimo/mimo-v2.5-pro` 解析正确
3. **模型元数据测试**: 验证 `get_model("mimo", "mimo-v2.5")` 返回正确元数据
4. **merge 测试**: 验证 `auth_header` 字段的显式覆盖和默认行为
5. **认证头构建测试**: 验证 `detect_openai_compat` 对非 Mimo provider 返回 `auth_header: None`（保持 `Authorization: Bearer` 不变）
6. **集成测试**（可选）: 若有 Mimo API key，可手动验证端到端调用
