# 修复 SSE StreamError 重试策略 — 区分瞬时网络错误与永久性错误

**Goal:** 将 `LlmError::StreamError` 从"统一不可重试"改为按错误类型区分：瞬时网络错误（broken pipe, connection reset, unexpected EOF）可重试，永久性错误（JSON parse failure, provider 明确 error 事件）不可重试。

**Architecture:** 细化 `StreamError` 变体，引入 `StreamErrorKind` 枚举区分 `Network` / `Protocol` / `Parse` 三类。`is_retryable()` 仅对 `Network` 类返回 true。所有生成点按语义标注正确 kind。

**Tech Stack:** Rust, llm-client crate

---

### 问题分析

当前 `LlmError::StreamError(String)` 是单一变体，所有场景统一不可重试：

| 生成位置 | 错误类型 | 当前 is_retryable | 实际应否重试 |
|---|---|---|---|
| `providers/anthropic.rs:209` | `eventsource_stream` SSE 解析/传输错误 | false | **true**（网络中断） |
| `providers/openai.rs:447` | `eventsource_stream` SSE 解析/传输错误 | false | **true**（网络中断） |
| `providers/mistral.rs:354` | `eventsource_stream` SSE 解析/传输错误 | false | **true**（网络中断） |
| `providers/google.rs:313` | `eventsource_stream` SSE 解析/传输错误 | false | **true**（网络中断） |
| `providers/bedrock.rs:267` | JSON parse error | false | **false**（格式错误） |
| `streaming.rs:177` | Provider 明确 Error SSE 事件 | false | **false**（业务逻辑错误） |
| `streaming.rs:186` | 流在收到 Done 前意外结束 | false | **true**（网络中断） |

**根本原因：** `eventsource_stream` 的错误和 `to_message()` 的"流意外结束"实际上是网络层瞬时故障，重试后很可能成功。当前策略导致这些错误直接传播到上层，造成不必要的失败。

---

### 方案：细化 StreamError 变体

将 `StreamError(String)` 重构为：

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamErrorKind {
    /// 瞬时网络/传输层错误（broken pipe, connection reset, unexpected EOF）
    Network,
    /// Provider 明确的协议级错误（Error SSE 事件）
    Protocol,
    /// 数据格式解析错误（JSON parse failure）
    Parse,
}

#[error("stream error ({kind:?}): {message}")]
StreamError { kind: StreamErrorKind, message: String },
```

`is_retryable()` 更新：
```rust
Self::StreamError { kind, .. } => matches!(kind, StreamErrorKind::Network),
```

---

### Task 1: 修改 error.rs 枚举定义

**Files:**
- Modify: `crates/llm-client/src/error.rs`

- [ ] **Step 1: 添加 StreamErrorKind 枚举，重构 StreamError 变体**

  在 `LlmError` enum 前添加 `StreamErrorKind`：
  ```rust
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum StreamErrorKind {
      Network,
      Protocol,
      Parse,
  }
  ```

  修改 `StreamError` 变体：
  ```rust
  #[error("stream error ({kind:?}): {message}")]
  StreamError { kind: StreamErrorKind, message: String },
  ```

- [ ] **Step 2: 更新 is_retryable()**

  ```rust
  pub fn is_retryable(&self) -> bool {
      matches!(
          self,
          Self::RateLimited(_) | Self::Overloaded(_) | Self::Timeout(_) |
          Self::StreamError { kind: StreamErrorKind::Network, .. }
      )
  }
  ```

- [ ] **Step 3: 更新 error.rs 中的测试**

  - `test_is_retryable_true_variants`：添加 `StreamError { kind: Network, message: "broken pipe".to_string() }`
  - `test_is_retryable_false_variants`：更新 `StreamError` 为 `StreamError { kind: Protocol, message: "broken pipe".to_string() }`

### Task 2: 更新所有 StreamError 生成点

**Files:**
- Modify: `crates/llm-client/src/providers/anthropic.rs:209`
- Modify: `crates/llm-client/src/providers/openai.rs:447`
- Modify: `crates/llm-client/src/providers/mistral.rs:354`
- Modify: `crates/llm-client/src/providers/google.rs:313`
- Modify: `crates/llm-client/src/providers/bedrock.rs:267`
- Modify: `crates/llm-client/src/streaming.rs:177,186`

- [ ] **Step 4: Provider SSE 传输错误 → Network**

  4 个 provider（anthropic, openai, mistral, google）统一改为：
  ```rust
  return Err(LlmError::StreamError {
      kind: StreamErrorKind::Network,
      message: format!("SSE stream error: {e}"),
  });
  ```

- [ ] **Step 5: Bedrock JSON parse → Parse**

  ```rust
  .map_err(|e| LlmError::StreamError {
      kind: StreamErrorKind::Parse,
      message: format!("JSON parse error: {e}"),
  })?;
  ```

- [ ] **Step 6: streaming.rs 区分 Protocol vs Network**

  line 177（明确 error 事件）：
  ```rust
  return Err(LlmError::StreamError {
      kind: StreamErrorKind::Protocol,
      message: error.error_message.unwrap_or_else(|| "stream terminated with error".to_string()),
  });
  ```

  line 186（流意外结束）：
  ```rust
  Err(LlmError::StreamError {
      kind: StreamErrorKind::Network,
      message: "stream ended without Done or Error".to_string(),
  })
  ```

### Task 3: 更新下游引用

**Files:**
- Modify: `crates/llm-client/src/streaming.rs` tests (line 378, 393)

- [ ] **Step 7: 更新 test 中的 match 模式**

  ```rust
  assert!(matches!(
      result.unwrap_err(),
      crate::LlmError::StreamError { kind: crate::StreamErrorKind::Protocol, .. }
  ));
  ```

  ```rust
  assert!(matches!(
      result.unwrap_err(),
      crate::LlmError::StreamError { kind: crate::StreamErrorKind::Network, .. }
  ));
  ```

### Task 4: 添加重试测试

**Files:**
- Modify: `crates/llm-client/src/retry.rs` tests

- [ ] **Step 8: 添加 StreamError::Network 重试测试**

  ```rust
  #[tokio::test]
  async fn test_retry_success_after_stream_error() {
      let counter = Arc::new(AtomicU32::new(0));
      let c = counter.clone();
      let result = with_retry(
          move || {
              let c = c.clone();
              async move {
                  let count = c.fetch_add(1, Ordering::SeqCst);
                  if count < 2 {
                      Err(LlmError::StreamError {
                          kind: StreamErrorKind::Network,
                          message: "connection reset".to_string(),
                      })
                  } else {
                      let (stream, _tx) = AssistantMessageEventStream::new(1);
                      Ok(stream)
                  }
              }
          },
          3,
          None,
      )
      .await;
      assert!(result.is_ok());
      assert_eq!(counter.load(Ordering::SeqCst), 3);
  }
  ```

  ```rust
  #[tokio::test]
  async fn test_no_retry_on_stream_error_protocol() {
      let counter = Arc::new(AtomicU32::new(0));
      let c = counter.clone();
      let result = with_retry(
          move || {
              let c = c.clone();
              async move {
                  c.fetch_add(1, Ordering::SeqCst);
                  Err(LlmError::StreamError {
                      kind: StreamErrorKind::Protocol,
                      message: "invalid model".to_string(),
                  })
              }
          },
          3,
          None,
      )
      .await;
      assert!(matches!(result, Err(LlmError::StreamError { kind: StreamErrorKind::Protocol, .. })));
      assert_eq!(counter.load(Ordering::SeqCst), 1);
  }
  ```

### Task 5: 验证

- [ ] **Step 9: 编译检查**

  Run: `cargo check -p llm-client`
  Expected: 零错误。

- [ ] **Step 10: 运行全部测试**

  Run: `cargo test -p llm-client -- --nocapture`
  Expected: 全部通过。

- [ ] **Step 11: Commit**

```bash
git add crates/llm-client/src/error.rs \
  crates/llm-client/src/providers/anthropic.rs \
  crates/llm-client/src/providers/openai.rs \
  crates/llm-client/src/providers/mistral.rs \
  crates/llm-client/src/providers/google.rs \
  crates/llm-client/src/providers/bedrock.rs \
  crates/llm-client/src/streaming.rs \
  crates/llm-client/src/retry.rs
git commit -m "fix(llm-client): distinguish retryable vs non-retryable StreamErrors

Replace monolithic StreamError(String) with StreamError { kind, message }
where kind is StreamErrorKind::Network | Protocol | Parse.

- Network errors (SSE transport failures, unexpected stream termination)
  are now retryable via is_retryable(), matching RateLimited/Overloaded/Timeout.
- Protocol errors (provider-sent Error SSE events) and Parse errors
  (JSON deserialization failures) remain non-retryable.

This prevents unnecessary failures from transient network interrupts
during streaming while preserving no-retry semantics for permanent
errors like invalid model IDs or malformed JSON."
```

---

## 变更范围

| 文件 | 变更 | 影响 |
|---|---|---|
| `error.rs` | 新增 `StreamErrorKind`，重构 `StreamError` 变体，更新 `is_retryable` | 核心语义变更 |
| `providers/anthropic.rs` | SSE 错误生成 Network kind | 网络中断可重试 |
| `providers/openai.rs` | SSE 错误生成 Network kind | 网络中断可重试 |
| `providers/mistral.rs` | SSE 错误生成 Network kind | 网络中断可重试 |
| `providers/google.rs` | SSE 错误生成 Network kind | 网络中断可重试 |
| `providers/bedrock.rs` | JSON parse 错误生成 Parse kind | 格式错误仍不可重试 |
| `streaming.rs` | 明确 error 事件 → Protocol，流意外结束 → Network | 正确分类 |
| `retry.rs` | 新增 StreamError Network/Protocol 重试测试 | 验证新行为 |

计划完成。是否立即执行？
