# 修复 provider 中的 unwrap_or_default() 序列化错误静默问题

**Goal:** 消除 LLM provider 中 `serde_json::to_string().unwrap_or_default()` 和 `response.text().await.unwrap_or_default()` 导致的静默失败，使序列化/IO 错误能被正确捕获并返回清晰的 `LlmError`。

**Architecture:** 两处修复：
1. **Tool call arguments 序列化**（openai.rs, mistral.rs）：将 `unwrap_or_default()` 改为 `map_err` + `?`，返回 `LlmError::Serialization`
2. **HTTP response body 读取**（anthropic.rs, google.rs, mistral.rs, openai.rs）：将 `unwrap_or_default()` 改为 `map_err`，保留原始 HTTP 错误上下文

**Tech Stack:** Rust, ai-provider crate

---

### 前置确认

搜索结果显示：
- **`serde_json::to_string(&tc.arguments).unwrap_or_default()`** 仅在 **openai.rs:53** 和 **mistral.rs:53** 两处（共 2 个 provider，非 4 个）
- **`response.text().await.unwrap_or_default()`** 在 **anthropic.rs:237, mistral.rs:173, google.rs:166, openai.rs:245** 四处（共 4 个 provider）

推测用户将两类问题统称为"4 个 provider 中的 unwrap() 问题"。本计划同时覆盖两类修复。

---

### Task 1: 修复 tool call arguments 序列化（OpenAI + Mistral）

**Files:**
- Modify: `crates/ai-provider/src/providers/openai.rs:53`
- Modify: `crates/ai-provider/src/providers/mistral.rs:53`

- [ ] **Step 1: OpenAI provider**

  将：
  ```rust
  "arguments": serde_json::to_string(&tc.arguments).unwrap_or_default()
  ```
  改为：
  ```rust
  "arguments": serde_json::to_string(&tc.arguments)
      .map_err(|e| LlmError::Serialization(format!("failed to serialize tool call arguments: {e}")))?
  ```

- [ ] **Step 2: Mistral provider**

  同 Step 1 的改动应用到 mistral.rs:53。

### Task 2: 修复 HTTP response body 读取（4 个 provider）

**Files:**
- Modify: `crates/ai-provider/src/providers/anthropic.rs:237`
- Modify: `crates/ai-provider/src/providers/google.rs:166`
- Modify: `crates/ai-provider/src/providers/mistral.rs:173`
- Modify: `crates/ai-provider/src/providers/openai.rs:245`

- [ ] **Step 3: 统一修复 response.text() 错误处理**

  各 provider 中类似模式：
  ```rust
  let body = response.text().await.unwrap_or_default();
  ```
  
  改为：
  ```rust
  let body = response.text().await
      .map_err(|e| LlmError::ProviderError(format!("failed to read response body: {e}")))?;
  ```

### Task 3: 验证

- [ ] **Step 4: 编译检查**

  Run: `cargo check -p ai-provider`
  Expected: 零错误。

- [ ] **Step 5: 运行 ai-provider 测试**

  Run: `cargo test -p ai-provider -- --nocapture`
  Expected: 全部通过。

- [ ] **Step 6: Commit**

```bash
git add crates/ai-provider/src/providers/
git commit -m "fix(ai-provider): replace unwrap_or_default with explicit error handling in providers

- Tool call arguments serialization (openai, mistral): errors now
  propagate as LlmError::Serialization instead of silently sending
  empty arguments to LLM APIs.
- HTTP response body reading (anthropic, google, mistral, openai):
  errors now propagate as LlmError::ProviderError with original
  context instead of silently producing empty bodies that fail
  JSON parsing downstream."
```

---

## 变更范围

| 文件 | 行 | 问题类型 | 修复方式 |
|---|---|---|---|
| `openai.rs` | 53 | `to_string().unwrap_or_default()` → 空 arguments | `map_err` → `LlmError::Serialization` + `?` |
| `mistral.rs` | 53 | `to_string().unwrap_or_default()` → 空 arguments | `map_err` → `LlmError::Serialization` + `?` |
| `anthropic.rs` | 237 | `response.text().unwrap_or_default()` → 空 body | `map_err` → `LlmError::ProviderError` + `?` |
| `google.rs` | 166 | `response.text().unwrap_or_default()` → 空 body | `map_err` → `LlmError::ProviderError` + `?` |
| `mistral.rs` | 173 | `response.text().unwrap_or_default()` → 空 body | `map_err` → `LlmError::ProviderError` + `?` |
| `openai.rs` | 245 | `response.text().unwrap_or_default()` → 空 body | `map_err` → `LlmError::ProviderError` + `?` |

计划完成。是否立即执行？
