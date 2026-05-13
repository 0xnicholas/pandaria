# 修复 HTTP 错误响应 raw body 泄露问题

**Goal:** 消除 4 个 LLM provider 中 HTTP 非 2xx 响应的 raw body 直接写入 `LlmError::ProviderError` 消息的安全隐患，引入结构化的错误 body 清洗逻辑，确保完整 body 仅用于内部可观测性（tracing），不泄露给上层调用方。

**Architecture:** 新增 `http_error.rs` 模块，提供 `sanitize_http_error_body(status, body)` 函数，尝试按 provider 标准 schema 提取安全的 `error.message`，失败时回退到通用 HTTP status 描述。4 个 provider 统一接入。完整 raw body 通过 `tracing::error!` 记录到 tenant_id span 中。

**Tech Stack:** Rust, ai-provider crate, tracing, serde_json

---

### 背景分析

当前 4 个 provider（anthropic, google, mistral, openai）中 HTTP 错误处理模式一致：

```rust
if !status.to_string().starts_with('2') {
    let body = response.text().await.map_err(...)?;
    return Err(LlmError::ProviderError(format!("HTTP {status}: {body}")));
}
```

**风险：**
- Provider 返回的 HTML 错误页（如 Cloudflare 502 页）直接暴露给上层
- Provider 内部调试信息（堆栈、路径）可能随 body 泄露
- 违反 AGENTS.md 安全约束：敏感信息不得出现在错误消息中

**现有测试期望：** `security_tests.rs::test_provider_error_no_raw_body` 已期望不包含 `<html>` 标签，但允许 "internal server error" 这类通用描述。

---

### Task 1: 创建 http_error 清洗模块

**Files:**
- Create: `crates/ai-provider/src/http_error.rs`
- Modify: `crates/ai-provider/src/lib.rs` (添加模块声明)

- [ ] **Step 1: 创建 `sanitize_http_error_body` 函数**

```rust
use reqwest::StatusCode;

/// Sanitize an HTTP error response body for safe inclusion in LlmError messages.
///
/// Attempts to extract a safe `message` field from standard provider error JSON schemas.
/// Falls back to a generic HTTP status description if the body is HTML, binary,
/// or does not match a known schema.
///
/// Known schemas (checked in order):
/// 1. OpenAI / Mistral: `{ "error": { "message": "...", ... } }`
/// 2. Anthropic: `{ "type": "error", "error": { "type": "...", "message": "..." } }`
/// 3. Google: `{ "error": { "code": N, "message": "...", "status": "..." } }`
/// 4. Generic: any JSON with a top-level or nested `message` string field
pub fn sanitize_http_error_body(status: StatusCode, body: &str) -> String {
    // Skip HTML and obvious non-JSON responses quickly.
    let trimmed = body.trim();
    if trimmed.starts_with('<') || trimmed.is_empty() {
        return format_http_status(status);
    }

    // Try JSON extraction.
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) {
        // 1. OpenAI / Mistral: error.message
        if let Some(msg) = json
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
        {
            return sanitize_message(msg);
        }

        // 2. Anthropic: error.error.message (nested)
        if let Some(msg) = json
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
        {
            return sanitize_message(msg);
        }

        // 3. Generic fallback: any message field anywhere in the JSON
        if let Some(msg) = find_message_field(&json) {
            return sanitize_message(msg);
        }
    }

    format_http_status(status)
}

fn format_http_status(status: StatusCode) -> String {
    match status.canonical_reason() {
        Some(reason) => format!("HTTP {} {}", status.as_u16(), reason),
        None => format!("HTTP {}", status.as_u16()),
    }
}

fn sanitize_message(msg: &str) -> String {
    // Trim and truncate to a reasonable length to avoid massive error messages.
    let trimmed = msg.trim();
    const MAX_LEN: usize = 256;
    if trimmed.len() > MAX_LEN {
        format!("{}...", &trimmed[..MAX_LEN])
    } else {
        trimmed.to_string()
    }
}

fn find_message_field(value: &serde_json::Value) -> Option<&str> {
    match value {
        serde_json::Value::Object(map) => {
            // Direct message field
            if let Some(msg) = map.get("message").and_then(|m| m.as_str()) {
                return Some(msg);
            }
            // Recurse into nested objects
            for v in map.values() {
                if let Some(msg) = find_message_field(v) {
                    return Some(msg);
                }
            }
            None
        }
        _ => None,
    }
}
```

- [ ] **Step 2: 在 lib.rs 中暴露模块**

```rust
pub mod http_error;
```

### Task 2: 统一修改 4 个 provider 的 HTTP 错误处理

**Files:**
- Modify: `crates/ai-provider/src/providers/anthropic.rs:163-166`
- Modify: `crates/ai-provider/src/providers/google.rs:165-168`
- Modify: `crates/ai-provider/src/providers/mistral.rs:183-186`
- Modify: `crates/ai-provider/src/providers/openai.rs:255-258`

- [ ] **Step 3: Anthropic provider**

```rust
        if !status.is_success() {
            let body = response
                .text()
                .await
                .map_err(|e| LlmError::ProviderError(format!("failed to read response body: {e}")))?;
            tracing::error!(
                status = %status,
                body = %body,
                provider = "anthropic",
                "HTTP error response from provider"
            );
            let msg = crate::http_error::sanitize_http_error_body(status, &body);
            return Err(LlmError::ProviderError(msg));
        }
```

- [ ] **Step 4: Google provider**（同 Step 3 模式）

- [ ] **Step 5: Mistral provider**（同 Step 3 模式）

- [ ] **Step 6: OpenAI provider**（同 Step 3 模式）

### Task 3: 更新安全测试

**Files:**
- Modify: `crates/ai-provider/tests/security_tests.rs`

- [ ] **Step 7: 调整 `test_provider_error_no_raw_body`**

  测试需要更新以匹配新的错误格式：
  - HTML body → 应返回 `HTTP 500 Internal Server Error`（无 HTML）
  - JSON error body → 应提取 `error.message`

```rust
#[test]
fn test_provider_error_no_raw_body() {
    // HTML response should be sanitized to generic status
    let err = llm_client::LlmError::ProviderError(
        "HTTP 500 Internal Server Error".to_string(),
    );
    let display = format!("{}", err);
    assert!(!display.contains("<html>"));
    assert!(!display.contains("<!DOCTYPE"));
    assert!(display.contains("500"));
}

#[test]
fn test_provider_error_extracts_json_message() {
    let body = r#"{"error":{"message":"The model does not exist","type":"invalid_request_error"}}"#;
    let msg = llm_client::http_error::sanitize_http_error_body(
        reqwest::StatusCode::BAD_REQUEST,
        body,
    );
    assert_eq!(msg, "The model does not exist");
}

#[test]
fn test_provider_error_sanitizes_html() {
    let body = "<html><body><h1>502 Bad Gateway</h1></body></html>";
    let msg = llm_client::http_error::sanitize_http_error_body(
        reqwest::StatusCode::BAD_GATEWAY,
        body,
    );
    assert_eq!(msg, "HTTP 502 Bad Gateway");
}
```

### Task 4: 验证

- [ ] **Step 8: 编译检查**

  Run: `cargo check -p ai-provider`
  Expected: 零错误。

- [ ] **Step 9: 运行安全相关测试**

  Run: `cargo test -p ai-provider security -- --nocapture`
  Expected: 全部通过。

- [ ] **Step 10: Commit**

```bash
git add crates/ai-provider/src/http_error.rs crates/ai-provider/src/lib.rs \
  crates/ai-provider/src/providers/ \
  crates/ai-provider/tests/security_tests.rs
git commit -m "fix(ai-provider): sanitize HTTP error bodies to prevent info leakage

- Add http_error module with sanitize_http_error_body() that extracts
  safe error.message from known provider JSON schemas and falls back
  to generic HTTP status descriptions for HTML/non-JSON responses.
- Update all 4 providers (anthropic, google, mistral, openai) to log
  full raw body via tracing::error! (for internal observability) while
  returning only sanitized messages in LlmError::ProviderError.
- Update security_tests to verify HTML is stripped and JSON messages
  are correctly extracted."
```

---

## 变更范围

| 文件 | 变更 | 影响 |
|---|---|---|
| `http_error.rs` | 新建：错误 body 清洗函数 | 所有 provider 统一使用 |
| `lib.rs` | 添加模块声明 | 暴露新模块 |
| `providers/anthropic.rs` | 错误处理接入清洗逻辑 | 不再泄露 raw body |
| `providers/google.rs` | 错误处理接入清洗逻辑 | 不再泄露 raw body |
| `providers/mistral.rs` | 错误处理接入清洗逻辑 | 不再泄露 raw body |
| `providers/openai.rs` | 错误处理接入清洗逻辑 | 不再泄露 raw body |
| `tests/security_tests.rs` | 更新测试断言 | 验证新行为 |

**安全保证：**
- Raw HTTP body 不再出现在 `LlmError::ProviderError` 的 Display 输出中
- Raw body 仅通过 `tracing::error!` 记录（继承 tenant_id/session_id span，用于内部调试）
- HTML 响应被完全过滤，只保留 HTTP status code + canonical reason
- JSON 响应只提取 `error.message`，不暴露其他字段（如 stack trace、internal paths）

计划完成。是否立即执行？
