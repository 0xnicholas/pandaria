# Mimo Provider 接入实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 ai-provider 新增小米 Mimo (`mimo-v2.5-pro`, `mimo-v2.5`) LLM 支持

**Architecture:** 复用 `OpenAiCompatibleProvider` + `openai_compatible_stream`。在 `OpenAiCompat` 新增 `auth_header` 字段（`None` 保持 `Authorization: Bearer`，`Some("api-key")` 只发 key 值），通过 `detect_openai_compat` 自动检测 Mimo 并设置。模型 compat 已在 `build_models` 中通过 `detect_openai_compat` 自动填充，`openai_compatible_stream` 直接从 model compat 读取。

**Tech Stack:** Rust, axum 生态

---

## 文件结构

| 文件 | 职责 |
|---|---|
| `crates/ai-provider/src/compat.rs` | `OpenAiCompat` + `auth_header` 字段；`detect_openai_compat` Mimo 检测；`merge_openai_compat` merge |
| `crates/ai-provider/src/providers/openai.rs` | `openai_compatible_stream` 读取 `compat.auth_header` 选择认证头 |
| `crates/ai-provider/src/resolver.rs` | `build_builtin_rules` 注册 `mimo` provider |
| `crates/ai-provider/src/models_data.rs` | 注册 `mimo-v2.5-pro`、`mimo-v2.5` + `build_provider_list` |

---

### Task 1: `OpenAiCompat` 新增 `auth_header` 字段

**Files:**
- Modify: `crates/ai-provider/src/compat.rs`

- [ ] **Step 1: 在 `OpenAiCompat` struct 末尾新增字段**

位置: `compat.rs` 第 51 行起 `pub struct OpenAiCompat {`

在 `vercel_gateway_routing` 字段之后（右花括号前）添加:

```rust
/// 覆盖默认认证头。
/// Some("api-key") → 请求带 `api-key: {key}`（无 Bearer 前缀）
/// None             → 保持默认 `Authorization: Bearer {key}`
#[serde(skip_serializing_if = "Option::is_none", default)]
pub auth_header: Option<String>,
```

- [ ] **Step 2: 编译确认 struct 变更无语法错误**

```bash
cd crates/ai-provider && cargo check 2>&1 | head -20
```

预期: 编译通过（`auth_header` 的 `Default` derive 自动给 `None`，现有代码无需修改）。

- [ ] **Step 3: Commit**

```bash
git add crates/ai-provider/src/compat.rs
git commit -m "feat(ai-provider): add auth_header field to OpenAiCompat"
```

---

### Task 2: `detect_openai_compat` 添加 Mimo 检测

**Files:**
- Modify: `crates/ai-provider/src/compat.rs`

- [ ] **Step 1: 添加 Mimo 检测变量**

位置: `detect_openai_compat` 函数内（约第 104 行，`let is_openrouter` 之后）:

```rust
let is_mimo = provider == "mimo" || base_url.contains("xiaomimimo.com");
```

- [ ] **Step 2: 在返回的 `OpenAiCompat` 中设置 `auth_header`**

位置: 函数末尾返回结构体（约第 133 行 `OpenAiCompat {`），在现有字段最后（`vercel_gateway_routing: None,` 之后、`}` 之前）添加:

```rust
auth_header: if is_mimo {
    Some("api-key".to_string())
} else {
    None
},
```

- [ ] **Step 3: 在 `merge_openai_compat` 中添加 merge 逻辑**

位置: 约第 173 行 `pub fn merge_openai_compat`，在现有字段列表末尾（`vercel_gateway_routing: opt_or(...)` 之后、右花括号前）添加:

```rust
auth_header: opt_or(&explicit.auth_header, &baseline.auth_header),
```

- [ ] **Step 4: 编译确认**

```bash
cd crates/ai-provider && cargo check 2>&1 | head -20
```

预期: 编译通过。

- [ ] **Step 5: 运行现有 compat 测试确认无回归**

```bash
cd crates/ai-provider && cargo test compat_tests -- --nocapture 2>&1 | tail -15
```

预期: 所有现有测试 PASS。

- [ ] **Step 6: 编写 `detect_openai_compat` Mimo 测试**

在 `compat.rs` 底部 `mod tests` 中添加:

```rust
#[test]
fn test_detect_mimo_auth_header() {
    let compat = detect_openai_compat(
        "mimo",
        "https://api.xiaomimimo.com/v1/chat/completions",
        "mimo-v2.5-pro",
    );
    assert_eq!(compat.auth_header, Some("api-key".to_string()));
}

#[test]
fn test_detect_non_mimo_auth_header_none() {
    let compat = detect_openai_compat(
        "openai",
        "https://api.openai.com/v1",
        "gpt-5.2",
    );
    assert_eq!(compat.auth_header, None);
}
```

- [ ] **Step 7: 运行新测试**

```bash
cd crates/ai-provider && cargo test test_detect_mimo_auth_header test_detect_non_mimo_auth_header_none -- --nocapture
```

预期: 两个测试 PASS。

- [ ] **Step 8: Commit**

```bash
git add crates/ai-provider/src/compat.rs
git commit -m "feat(ai-provider): detect Mimo auth_header in detect_openai_compat"
```

---

### Task 3: `openai_compatible_stream` 使用 `compat.auth_header`

**Files:**
- Modify: `crates/ai-provider/src/providers/openai.rs`

- [ ] **Step 1: 替换硬编码认证头**

位置: `openai.rs` 第 219-225 行，`openai_compatible_stream` 函数内:

```rust
// 变更前 (第 219-225 行):
    let mut builder =
        crate::protocol::request::RequestBuilder::new(client, base_url, fallback, options.clone())
            .body(body)
            .header(
                "Authorization",
                format!("Bearer {}", api_key.expose_secret()),
            );

// 变更后:
    let mut builder =
        crate::protocol::request::RequestBuilder::new(client, base_url, fallback, options.clone())
            .body(body);

    let (auth_key, auth_value) = match compat.auth_header.as_ref() {
        Some(header_name) => (header_name.as_str(), api_key.expose_secret().to_string()),
        None => (
            "Authorization",
            format!("Bearer {}", api_key.expose_secret()),
        ),
    };
    builder = builder.header(auth_key, auth_value);
```

> 注：`compat` 变量在函数中已存在（约第 168 行用于 thinking/reasoning），类型为 `OpenAiCompat`。

- [ ] **Step 2: 编译确认**

```bash
cd crates/ai-provider && cargo check 2>&1 | head -20
```

预期: 编译通过。

- [ ] **Step 3: 运行现有 openai 测试确认无回归**

```bash
cd crates/ai-provider && cargo test openai -- --nocapture 2>&1 | tail -15
```

预期: 所有现有测试 PASS（现有 compat 都不设 `auth_header`，走 `None` 分支即原有行为）。

- [ ] **Step 4: Commit**

```bash
git add crates/ai-provider/src/providers/openai.rs
git commit -m "feat(ai-provider): use compat.auth_header for auth header selection"
```

---

### Task 4: 注册模型元数据

**Files:**
- Modify: `crates/ai-provider/src/models_data.rs`

- [ ] **Step 1: 在 `build_models` 中注册 Mimo 模型**

位置: Google section 之后、`m` 返回之前（约第 670 行，`// Fill compat fields` 前），添加:

```rust
    // ── Mimo ────────────────────────────────────────────────────────
    insert!(
        m,
        "mimo",
        "mimo-v2.5-pro",
        "MiMo V2.5 Pro",
        "openai-completions",
        "https://api.xiaomimimo.com/v1/chat/completions",
        true,
        vec![Modality::Text],
        TokenCost {
            input: 0.20,
            output: 1.00,
            cache_read: 0.0,
            cache_write: 0.0
        },
        1_048_576,
        128_000
    );
    insert!(
        m,
        "mimo",
        "mimo-v2.5",
        "MiMo V2.5",
        "openai-completions",
        "https://api.xiaomimimo.com/v1/chat/completions",
        true,
        vec![Modality::Text],
        TokenCost {
            input: 0.08,
            output: 0.40,
            cache_read: 0.0,
            cache_write: 0.0
        },
        1_048_576,
        128_000
    );
```

- [ ] **Step 2: 在 `build_provider_list` 中注册 Mimo provider 模型列表**

位置: Doubao 条目之后（约第 784 行），添加:

```rust
    p.insert(
        "mimo".to_string(),
        vec![
            "mimo-v2.5-pro".to_string(),
            "mimo-v2.5".to_string(),
        ],
    );
```

- [ ] **Step 3: 运行模型元数据测试**

```bash
cd crates/ai-provider && cargo test models_tests -- --nocapture 2>&1 | tail -15
```

预期: 所有测试 PASS（包括 `test_provider_consistency`、`test_all_provider_models_exist` 等交叉验证）。

- [ ] **Step 4: Commit**

```bash
git add crates/ai-provider/src/models_data.rs
git commit -m "feat(ai-provider): add Mimo v2.5 model metadata"
```

---

### Task 5: 注册 Mimo Provider 规则

**Files:**
- Modify: `crates/ai-provider/src/resolver.rs`

- [ ] **Step 1: 在 `build_builtin_rules` 中注册 `mimo`**

位置: Ollama 条目之后（约第 312 行 `rules` 的 `});` 之前），添加:

```rust
        rules.insert(
            "mimo".to_string(),
            ProviderRule {
                factory: ProviderFactory::OpenAiCompatible {
                    provider_name: "mimo".to_string(),
                    env_key: "MIMO_API_KEY",
                },
                default_base_url: "https://api.xiaomimimo.com/v1/chat/completions"
                    .to_string(),
                env_key: "MIMO_API_KEY",
                api_type: "openai-completions",
                compat_hints: None,
                fallback_context_window: 1_048_576,
                fallback_max_tokens: 128_000,
            },
        );
```

- [ ] **Step 2: 添加 resolver 单元测试**

在 `resolver.rs` 底部 `mod tests` 中添加:

```rust
    #[test]
    fn test_resolve_mimo() {
        let resolver = ProviderResolver::new();
        let resolved = resolver.resolve("mimo/mimo-v2.5-pro").unwrap();
        assert_eq!(resolved.provider_name, "mimo");
        assert_eq!(resolved.model_id, "mimo-v2.5-pro");
        assert_eq!(resolved.api_type, "openai-completions");
    }
```

- [ ] **Step 3: 运行 resolver 测试**

```bash
cd crates/ai-provider && cargo test resolver -- --nocapture 2>&1 | tail -15
```

预期: 所有测试 PASS（包括新测试）。

- [ ] **Step 4: Commit**

```bash
git add crates/ai-provider/src/resolver.rs
git commit -m "feat(ai-provider): register Mimo provider in resolver"
```

---

### Task 6: 全量回归测试

**Files:** None (仅测试)

- [ ] **Step 1: 运行 ai-provider 全部单元测试**

```bash
cd crates/ai-provider && cargo test --lib -- --nocapture 2>&1 | tail -20
```

预期: 全部 PASS，无回归。

- [ ] **Step 2: 运行完整编译**

```bash
cargo check --workspace 2>&1 | tail -10
```

预期: 编译通过。

---

## 验证清单

- [ ] `mimo/mimo-v2.5-pro` spec 可解析
- [ ] `mimo/mimo-v2.5` spec 可解析
- [ ] `get_model("mimo", "mimo-v2.5-pro")` 返回正确元数据
- [ ] Mimo 模型的 `compat.auth_header = Some("api-key")`
- [ ] 非 Mimo provider 的 `compat.auth_header = None`（保持 `Authorization: Bearer`）
- [ ] 全部现有测试无回归
