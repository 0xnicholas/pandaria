# llm-client Code Review Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Address 12 issues identified in the llm-client code review: eliminate provider boilerplate duplication, fix bugs (TOCTOU, dead code, no-op test), improve API design (OAuth error types, streaming parser, transform signature), and add missing tests/docs.

> Note: Issue #4 (`expect` in provider constructors) is already ADR-compliant — the ADR says to use `?` or explicit `expect("reason")`. The existing code uses `expect("reqwest client should build")` with a reason, so no fix needed.

**Architecture:** Extract shared provider structure into a `providers/shared.rs` macro. Fix `OAuthProvider` error types to use `LlmError`. Make all provider `models()` read from the centralized `models_data.rs`. Add request body validation tests using wiremock.

**Tech Stack:** Rust 2024 edition, tokio, reqwest+wiremock, serde, secrecy, jsonschema

---

## File Map

### Files to Modify
| File | Changes |
|---|---|
| `crates/llm-client/src/cache.rs` | Rename env var `PI_CACHE_RETENTION` → `PANDARIA_CACHE_RETENTION` |
| `crates/llm-client/src/provider.rs` | Remove dead `XHigh` arm in `adjust_max_tokens_for_thinking` |
| `crates/llm-client/src/providers/bedrock.rs` | Remove meaningless `test_provider_name_static` test |
| `crates/llm-client/src/util.rs` | Remove `build_tool_defs` function |
| `crates/llm-client/src/lib.rs` | Remove `build_tool_defs` re-export; update `OAuthProvider` re-exports |
| `crates/llm-client/src/validation.rs` | Fix TOCTOU race in `validate_tool_arguments` cache logic |
| `crates/llm-client/src/transform.rs` | Change `transform_messages` signature to `Vec<Message>`; update tests |
| `crates/llm-client/tests/transform_tests.rs` | Update to pass `Vec<Message>` |
| `crates/llm-client/src/repair.rs` | Add `peek_value()` to `StreamingJsonParser` for incremental parse |
| `crates/llm-client/src/oauth.rs` | Change `OAuthProvider` trait errors from `io::Error` to `LlmError`; simplify `resolve_oauth_key` signature |
| `crates/llm-client/src/providers/anthropic.rs` | Replace boilerplate with macro; add docs; update `resolve_api_key`; fix `models()` |
| `crates/llm-client/src/providers/openai.rs` | Replace boilerplate with macro; add docs; update `resolve_api_key`; fix `models()` |
| `crates/llm-client/src/providers/google.rs` | Replace boilerplate with macro; add docs; update `resolve_api_key`; fix `models()` |
| `crates/llm-client/src/providers/mistral.rs` | Replace boilerplate with macro; add docs; update `resolve_api_key`; fix `models()` |
| `crates/llm-client/src/providers/mod.rs` | Add `shared` module |
| `crates/llm-client/src/models.rs` | Add `models_for_provider_names()` returning `Vec<String>` (model IDs) |

### New Files to Create
| File | Responsibility |
|---|---|
| `crates/llm-client/src/providers/shared.rs` | Macro + helpers for shared provider boilerplate |
| `crates/llm-client/tests/provider_requests.rs` | Request body validation tests for each provider |

---

## Dependencies

- **Task 9 (macro)** must be completed before **Task 10 (models from data)** and **Task 11 (OAuth)** because the macro generates `resolve_api_key` and `LlmProvider` impl.
- **Tasks 1-5** are independent and can be done in any order.
- **Tasks 6-8** modify one file each, independent of each other.
- **Task 12 (tests)** depends on Task 9 (macro) for the provider structure to be stable.

---

### Task 1: Rename env var `PI_CACHE_RETENTION` → `PANDARIA_CACHE_RETENTION`

**Files:** Modify: `crates/llm-client/src/cache.rs:15,53`

- [ ] **Step 1: Rename the env var string**

```rust
// In cache.rs, change:
std::env::var("PI_CACHE_RETENTION")
// To:
std::env::var("PANDARIA_CACHE_RETENTION")
```

Also update the test comment on line 53.

- [ ] **Step 2: Run tests to verify**

Run: `cargo test -p llm-client cache`
Expected: 3 tests PASS

- [ ] **Step 3: Commit**

```bash
git add crates/llm-client/src/cache.rs
git commit -m "fix: rename cache retention env var to PANDARIA_CACHE_RETENTION"
```

---

### Task 2: Fix dead code in `adjust_max_tokens_for_thinking`

**Files:** Modify: `crates/llm-client/src/provider.rs:43-55`

The `XHigh` arm at line 54 is unreachable because lines 43-47 map `XHigh` to `High` before the match. Remove the mapping and handle `XHigh` directly.

- [ ] **Step 1: Replace the dead-code section**

```rust
// Replace lines 42-55:
    let budgets = custom_budgets.unwrap_or(&default_budgets);
    // Remove the XHigh → High mapping block (lines 43-47)

    let thinking_budget = match reasoning_level {
        ReasoningLevel::Minimal => budgets.minimal.unwrap_or(1024),
        ReasoningLevel::Low => budgets.low.unwrap_or(2048),
        ReasoningLevel::Medium => budgets.medium.unwrap_or(8192),
        ReasoningLevel::High => budgets.high.unwrap_or(16384),
        ReasoningLevel::XHigh => budgets.high.unwrap_or(16384),
    };
```

Note: XHigh uses the same budget as High (no separate XHigh budget field), but the logic is now explicit and reachable. For XHigh, we should also consider: if custom budgets provide a `high` field, use it, otherwise default to 16384.

- [ ] **Step 2: Update the XHigh test to match new behavior**

The test `test_adjust_tokens_xhigh_clamped` at line 311 expects `max_tokens == 20480` and `thinking_budget == 16384`. This should still pass since XHigh uses `high` budget (16384 default). Verify.

Run: `cargo test -p llm-client adjust_tokens`
Expected: 4 tests PASS

- [ ] **Step 3: Commit**

```bash
git add crates/llm-client/src/provider.rs
git commit -m "fix: remove dead XHigh arm in adjust_max_tokens_for_thinking"
```

---

### Task 3: Fix Bedrock no-op test

**Files:** Modify: `crates/llm-client/src/providers/bedrock.rs:69-78`

- [ ] **Step 1: Remove the meaningless test**

Delete the entire `#[cfg(test)] mod tests` block (lines 69-78). The `test_provider_name_static` test asserts `"bedrock" == "bedrock"` which tests nothing.

- [ ] **Step 2: Verify the file still compiles**

Run: `cargo check -p llm-client --features bedrock`
Expected: success

- [ ] **Step 3: Commit**

```bash
git add crates/llm-client/src/providers/bedrock.rs
git commit -m "fix: remove meaningless bedrock provider test"
```

---

### Task 4: Remove `build_tool_defs` thin wrapper

**Files:**
- Modify: `crates/llm-client/src/util.rs:14-18, 54-70`
- Modify: `crates/llm-client/src/lib.rs:43`

`build_tool_defs` is just `tools.to_vec()` with a semantic name. No external callers exist.

- [ ] **Step 1: Remove function and its test from util.rs**

Delete lines 14-18 (the function) and lines 54-70 (the `test_build_tool_defs` test).

- [ ] **Step 2: Remove re-export from lib.rs**

Change line 43 from:
```rust
pub use util::{build_tool_defs, extract_tool_calls};
```
To:
```rust
pub use util::extract_tool_calls;
```

- [ ] **Step 3: Verify tests pass**

Run: `cargo test -p llm-client util`
Expected: 2 tests PASS (extract_tool_calls tests)

- [ ] **Step 4: Commit**

```bash
git add crates/llm-client/src/util.rs crates/llm-client/src/lib.rs
git commit -m "refactor: remove build_tool_defs thin wrapper"
```

---

### Task 5: Add missing docs to provider structs and public methods

**Files:** Modify: `crates/llm-client/src/providers/{anthropic,openai,google,mistral}.rs`

Add `///` doc comments to each provider struct, constructor, and public method.

- [ ] **Step 1: Add docs to AnthropicProvider**

Add to `anthropic.rs`:
```rust
/// Anthropic Claude provider via the Messages API.
///
/// Supports SSE streaming, cache control, adaptive thinking,
/// and fine-grained tool input streaming for Claude 4+ models.
/// Requires `ANTHROPIC_API_KEY` environment variable or explicit API key.
pub struct AnthropicProvider { ... }
```

Add docs to `new`, `with_base_url`, `with_oauth`:
```rust
/// Create a new Anthropic provider with the default API endpoint.
pub fn new(api_key: Option<SecretString>) -> Self { ... }

/// Create a new Anthropic provider with a custom base URL.
pub fn with_base_url(api_key: Option<SecretString>, base_url: &str) -> Self { ... }

/// Attach an OAuth provider for automatic token management.
pub fn with_oauth(mut self, oauth: std::sync::Arc<dyn crate::oauth::OAuthProvider>) -> Self { ... }
```

- [ ] **Step 2: Repeat for OpenAiProvider, GoogleProvider, MistralProvider**

Use the same pattern, adjusting the description text per provider.

- [ ] **Step 3: Verify clippy is still clean**

Run: `cargo clippy -p llm-client -- -D warnings`
Expected: success

- [ ] **Step 4: Commit**

```bash
git add crates/llm-client/src/providers/
git commit -m "docs: add doc comments to provider structs and constructors"
```

---

### Task 6: Fix TOCTOU race in `validate_tool_arguments`

**Files:** Modify: `crates/llm-client/src/validation.rs:124-188`

The current code checks the cache under a lock (lines 133-159), drops the lock, then acquires it again to write (lines 173-176). Two concurrent calls could both miss and compile separately.

- [ ] **Step 1: Rewrite to hold lock across check + insert**

Replace the entire `validate_tool_arguments` function body (lines 129-188) with:

```rust
pub fn validate_tool_arguments(
    tool: &ToolDef,
    tool_call: &ToolCall,
) -> Result<serde_json::Value, ValidationError> {
    let mut args = tool_call.arguments.clone();
    coerce_value(&mut args, &tool.parameters);

    // Check cache, compiling and caching if not present.
    // Hold lock across check + insert to avoid TOCTOU race.
    let compiled = {
        let mut cache = SCHEMA_CACHE.lock().expect("schema cache lock poisoned");
        if let Some(cached) = cache.get(&tool.name) {
            if cached == &tool.parameters {
                // Schema unchanged — reuse cached validator
                jsonschema::validator_for(cached).map_err(|e| {
                    ValidationError::schema_violation(
                        tool.name.clone(),
                        vec![ValidationMessage {
                            path: String::new(),
                            message: format!("schema compilation error: {e}"),
                        }],
                        args.clone(),
                    )
                })?
            } else {
                // Schema changed — recompile
                let compiled = jsonschema::validator_for(&tool.parameters).map_err(|e| {
                    ValidationError::schema_violation(
                        tool.name.clone(),
                        vec![ValidationMessage {
                            path: String::new(),
                            message: format!("schema compilation error: {e}"),
                        }],
                        args.clone(),
                    )
                })?;
                cache.insert(tool.name.clone(), tool.parameters.clone());
                compiled
            }
        } else {
            // Not in cache — compile and store
            let compiled = jsonschema::validator_for(&tool.parameters).map_err(|e| {
                ValidationError::schema_violation(
                    tool.name.clone(),
                    vec![ValidationMessage {
                        path: String::new(),
                        message: format!("schema compilation error: {e}"),
                    }],
                    args.clone(),
                )
            })?;
            cache.insert(tool.name.clone(), tool.parameters.clone());
            compiled
        }
    };

    if compiled.is_valid(&args) {
        return Ok(args);
    }

    let errors = ValidationError::collect_errors(&compiled, &args);
    Err(ValidationError::schema_violation(
        tool.name.clone(),
        errors,
        args,
    ))
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p llm-client validation`
Expected: 8 tests PASS (6 unit + 8 integration in validation_tests.rs = 14 total)

- [ ] **Step 3: Verify no dead imports**

Check that removing the duplicate `collect_errors` and `schema_violation` methods from the impl block doesn't leave unused items. The `ValidationError` impl block (lines 190-225) should remain unchanged.

- [ ] **Step 4: Commit**

```bash
git add crates/llm-client/src/validation.rs
git commit -m "fix: TOCTOU race in validate_tool_arguments schema cache"
```

---

### Task 7: Fix `transform_messages` signature

**Files:**
- Modify: `crates/llm-client/src/transform.rs:21, 38, 239, 275, 298, 345, 386`
- Modify: `crates/llm-client/tests/transform_tests.rs:62, 85, 110, 145, 168, 196, 218, 237`

The function takes `&[Message]` but immediately clones to `messages.to_vec()`. Change to `Vec<Message>` to be explicit about ownership. No external callers exist.

- [ ] **Step 1: Change signature in transform.rs**

```rust
// Before:
pub fn transform_messages(messages: &[Message], options: &TransformOptions) -> Vec<Message> {
    let mut result: Vec<Message> = messages.to_vec();
// After:
pub fn transform_messages(messages: Vec<Message>, options: &TransformOptions) -> Vec<Message> {
    let mut result: Vec<Message> = messages;
```

- [ ] **Step 2: Update all callers in transform.rs unit tests**

Change all test calls from `transform_messages(&messages, &opts)` to `transform_messages(messages, &opts)`. This affects ~7 call sites (lines 239, 275, 298, 345, 386 + possible others). Note: `messages` is already owned in each test, so just remove the `&`.

- [ ] **Step 3: Update integration test file**

In `tests/transform_tests.rs`, change all 8 call sites from `transform_messages(&messages, ...)` to `transform_messages(messages, ...)`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p llm-client transform`
Expected: all transform tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/llm-client/src/transform.rs crates/llm-client/tests/transform_tests.rs
git commit -m "refactor: take Vec<Message> by value in transform_messages"
```

---

### Task 8: Add incremental parse to `StreamingJsonParser`

**Files:** Modify: `crates/llm-client/src/repair.rs:13-41`

Add a `peek_value()` method that returns the current best-effort parse result without consuming the parser, enabling consumers to progressively validate tool call arguments during streaming.

- [ ] **Step 1: Add `peek_value` method**

```rust
impl StreamingJsonParser {
    // ... existing methods ...

    /// Return the current best-effort parsed value without consuming
    /// the parser. Returns `None` if nothing has been fed yet.
    pub fn peek_value(&self) -> Option<serde_json::Value> {
        if self.buffer.is_empty() {
            return None;
        }
        parse_json_with_repair(&self.buffer).ok()
    }
}
```

- [ ] **Step 2: Add test for peek_value**

Add to the test module in repair.rs:

```rust
#[test]
fn test_streaming_parser_peek_progressive() {
    let mut parser = StreamingJsonParser::new();
    assert!(parser.peek_value().is_none());

    parser.feed(r#"{"ke"#);
    // Best-effort: may or may not parse depending on repair heuristics
    // At minimum, peek_value should not panic

    parser.feed(r#"y": "va"#);
    // Still partial

    parser.feed(r#"lue"}"#);
    let val = parser.peek_value().unwrap();
    assert_eq!(val["key"], "value");

    // finalize should also work
    let val2 = parser.finalize().unwrap();
    assert_eq!(val2["key"], "value");
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p llm-client repair`
Expected: all repair tests PASS (now ~13 tests)

- [ ] **Step 4: Commit**

```bash
git add crates/llm-client/src/repair.rs
git commit -m "feat: add peek_value to StreamingJsonParser for incremental parse"
```

---

### Task 9: Extract shared provider boilerplate via macro

**Files:**
- Create: `crates/llm-client/src/providers/shared.rs`
- Modify: `crates/llm-client/src/providers/mod.rs`
- Modify: `crates/llm-client/src/providers/anthropic.rs`
- Modify: `crates/llm-client/src/providers/openai.rs`
- Modify: `crates/llm-client/src/providers/google.rs`
- Modify: `crates/llm-client/src/providers/mistral.rs`

This is the largest task. It creates a macro that generates the struct, constructors, `resolve_api_key` (with unified OAuth + static key chain), `LlmProvider` trait impl, and `stream()` method. Each provider file then invokes the macro and defines only `try_stream`.

- [ ] **Step 1: Create `providers/shared.rs` with the macro**

```rust
/// Shared provider boilerplate macro.
///
/// Generates the provider struct, constructors, unified key resolution
/// (OAuth → options → instance → env var), and the `LlmProvider` trait
/// implementation. The per-provider `try_stream` function is expected
/// to be defined as an associated function on the generated struct.
macro_rules! define_provider {
    (
        $struct_name:ident,
        $provider_str:literal,
        $env_key:literal,
        $default_url:literal
    ) => {
        #[doc = concat!("Provider for the ", $provider_str, " API.")]
        pub struct $struct_name {
            client: reqwest::Client,
            api_key: Option<secrecy::SecretString>,
            base_url: String,
            oauth_provider: Option<std::sync::Arc<dyn crate::oauth::OAuthProvider>>,
        }

        impl $struct_name {
            #[doc = concat!("Create a new ", $provider_str, " provider with the default endpoint.")]
            pub fn new(api_key: Option<secrecy::SecretString>) -> Self {
                Self::with_base_url(api_key, $default_url)
            }

            #[doc = concat!("Create a new ", $provider_str, " provider with a custom base URL.")]
            pub fn with_base_url(
                api_key: Option<secrecy::SecretString>,
                base_url: &str,
            ) -> Self {
                let client = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(60))
                    .build()
                    .expect("reqwest client should build");
                Self {
                    client,
                    api_key,
                    base_url: base_url.to_string(),
                    oauth_provider: None,
                }
            }

            /// Attach an OAuth provider for automatic token management.
            pub fn with_oauth(
                mut self,
                oauth: std::sync::Arc<dyn crate::oauth::OAuthProvider>,
            ) -> Self {
                self.oauth_provider = Some(oauth);
                self
            }

            /// Resolve the API key from all available sources.
            ///
            /// Fallback chain: OAuth token → StreamOptions.api_key →
            /// instance api_key → environment variable.
            async fn resolve_api_key(
                &self,
                options: &crate::provider::StreamOptions,
            ) -> Result<secrecy::SecretString, crate::error::LlmError> {
                if let Some(key) =
                    crate::oauth::resolve_oauth_key(&self.oauth_provider).await
                {
                    return Ok(key);
                }
                if let Some(key) = &options.api_key {
                    return Ok(key.clone());
                }
                if let Some(key) = &self.api_key {
                    return Ok(key.clone());
                }
                if let Ok(key) = std::env::var($env_key) {
                    return Ok(secrecy::SecretString::new(key.into_boxed_str()));
                }
                Err(crate::error::LlmError::AuthError(format!(
                    "{} not set",
                    $env_key
                )))
            }

            /// Reference to the HTTP client.
            pub fn client(&self) -> &reqwest::Client {
                &self.client
            }

            /// Reference to the base URL.
            pub fn base_url(&self) -> &str {
                &self.base_url
            }
        }

        #[async_trait::async_trait]
        impl crate::provider::LlmProvider for $struct_name {
            fn provider_name(&self) -> &str {
                $provider_str
            }

            fn models(&self) -> Vec<String> {
                crate::models::models_for_provider($provider_str)
                    .into_iter()
                    .map(|m| m.id)
                    .collect()
            }

            async fn stream(
                &self,
                model: &str,
                context: crate::types::LlmContext,
                options: crate::provider::StreamOptions,
                signal: tokio_util::sync::CancellationToken,
            ) -> Result<
                crate::streaming::AssistantMessageEventStream,
                crate::error::LlmError,
            > {
                let api_key = self.resolve_api_key(&options).await?;
                let (stream, tx) =
                    crate::streaming::AssistantMessageEventStream::new(32);
                let client = self.client.clone();
                let model = model.to_string();
                let base_url = self.base_url().to_string();
                let provider_name = self.provider_name().to_string();

                tokio::spawn(async move {
                    let result = Self::try_stream(
                        client,
                        base_url,
                        &model,
                        context,
                        options,
                        &tx,
                        api_key,
                        signal,
                    )
                    .await;
                    if let Err(e) = result {
                        let err_msg = e.to_string();
                        let _ = tx
                            .send(crate::streaming::AssistantMessageEvent::Error {
                                error: crate::types::AssistantMessage {
                                    content: vec![],
                                    provider: provider_name.clone(),
                                    model: model.clone(),
                                    api: crate::types::Api {
                                        provider: provider_name,
                                        model: model.clone(),
                                    },
                                    usage: crate::types::Usage::default(),
                                    stop_reason: crate::types::StopReason::Error,
                                    response_id: None,
                                    error_message: Some(format!(
                                        "{provider} '{model}' {api}: {err_msg}",
                                        provider = $provider_str,
                                        model = model,
                                        api = model,
                                        err_msg = err_msg,
                                    )),
                                    timestamp: std::time::SystemTime::now(),
                                },
                            })
                            .await;
                    }
                });
                Ok(stream)
            }
        }
    };
}

pub(crate) use define_provider;
```

- [ ] **Step 2: Register the module in providers/mod.rs**

Add before the existing module declarations:
```rust
#[macro_use]
mod shared;
```

- [ ] **Step 3: Rewrite AnthropicProvider to use the macro**

Replace the entire anthropic.rs down to just before `try_stream`:

```rust
use secrecy::{ExposeSecret, SecretString};
use tokio_util::sync::CancellationToken;

use crate::error::LlmError;
use crate::streaming::{AssistantMessageEvent, AssistantMessageEventStream};
use crate::types::{Api, LlmContext};

crate::providers::shared::define_provider!(
    AnthropicProvider,
    "anthropic",
    "ANTHROPIC_API_KEY",
    "https://api.anthropic.com/v1/messages"
);

// Delete: struct AnthropicProvider { ... }
// Delete: impl AnthropicProvider { new, with_base_url, with_oauth, resolve_api_key }
// Delete: #[async_trait] impl LlmProvider for AnthropicProvider { ... }
// Keep: impl AnthropicProvider { async fn try_stream(...) } and all helper functions
```

Then remove the `use async_trait::async_trait;` line since the macro imports it internally. Remove unused imports (`use crate::oauth::resolve_oauth_key;`, `use crate::provider::LlmProvider;`).

**Important:** Any call to `self.client` inside `try_stream` must change to `self.client()` (the accessor method the macro generates). Similarly `self.base_url` → `self.base_url()`.

- [ ] **Step 4: Repeat for OpenAiProvider**

Same pattern:
```rust
crate::providers::shared::define_provider!(
    OpenAiProvider,
    "openai",
    "OPENAI_API_KEY",
    "https://api.openai.com/v1/chat/completions"
);
```

- [ ] **Step 5: Repeat for GoogleProvider**

```rust
crate::providers::shared::define_provider!(
    GoogleProvider,
    "google",
    "GOOGLE_API_KEY",
    "https://generativelanguage.googleapis.com/v1beta"
);
```

- [ ] **Step 6: Repeat for MistralProvider**

```rust
crate::providers::shared::define_provider!(
    MistralProvider,
    "mistral",
    "MISTRAL_API_KEY",
    "https://api.mistral.ai/v1/chat/completions"
);
```

- [ ] **Step 7: Compile and fix errors**

Run: `cargo check -p llm-client 2>&1`
Expected: check for compilation errors, fix any that arise (likely `self.client` → `self.client()` accessor renames, import cleanup)

- [ ] **Step 8: Run full test suite**

Run: `cargo test -p llm-client`
Expected: all ~184 tests PASS

- [ ] **Step 9: Run clippy**

Run: `cargo clippy -p llm-client -- -D warnings`
Expected: success

- [ ] **Step 10: Commit**

```bash
git add crates/llm-client/src/providers/shared.rs crates/llm-client/src/providers/mod.rs crates/llm-client/src/providers/anthropic.rs crates/llm-client/src/providers/openai.rs crates/llm-client/src/providers/google.rs crates/llm-client/src/providers/mistral.rs
git commit -m "refactor: extract shared provider boilerplate into macro"
```

---

### Task 10: Fix `models()` to delegate to `models_data.rs`

**Files:**
- Modify: `crates/llm-client/src/models.rs`
- Modify: `crates/llm-client/src/providers/anthropic.rs`
- Modify: `crates/llm-client/src/providers/openai.rs`
- Modify: `crates/llm-client/src/providers/google.rs`
- Modify: `crates/llm-client/src/providers/mistral.rs`

The macro in Task 9 already generates `models()` that reads from `ModelRegistry`. But each provider currently has a separate unit test checking hardcoded model lists. Those tests will now return different counts (from `models_data.rs`). Also, model lists in `models_data.rs` may need updating.

- [ ] **Step 1: Verify models_data.rs has complete model lists**

Check that `models_data.rs` contains all models that were previously hardcoded in provider `models()` methods:

| Provider | Old hardcoded (per impl) | models_data.rs | Status |
|---|---|---|---|
| anthropic | 3 (sonnet, opus, haiku) | 6 | More complete in data |
| openai | 3 (gpt-5.2, 5.1, 4.1) | 8 | More complete in data |
| google | 3 (pro, flash, flash) | 4 | More complete in data |
| mistral | 2 (large, medium) | 2 | Same |

`models_data.rs` already has more models per provider, so this is an upgrade.

- [ ] **Step 2: Update unit tests that check model counts**

Each provider has inline tests like:
```rust
fn test_models() {
    let p = AnthropicProvider::new(None);
    assert_eq!(p.models(), vec!["claude-sonnet-4-20250514", ...]);
}
```

These need to be updated since `models()` now returns all models from `models_data.rs`. Replace exact vec comparisons with more flexible assertions:

```rust
fn test_models() {
    let p = AnthropicProvider::new(None);
    let m = p.models();
    assert!(m.contains(&"claude-sonnet-4-20250514".to_string()));
    assert!(m.contains(&"claude-opus-4-7".to_string()));
    assert!(m.len() >= 3);
}
```

Do this for all four provider test modules.

- [ ] **Step 3: Run tests**

Run: `cargo test -p llm-client`
Expected: all tests PASS

- [ ] **Step 4: Commit**

```bash
git add crates/llm-client/src/providers/
git commit -m "fix: delegate provider models() to centralized models_data.rs"
```

---

### Task 11: Fix OAuthProvider error types and `resolve_oauth_key` signature

**Files:**
- Modify: `crates/llm-client/src/oauth.rs`
- Modify: `crates/llm-client/src/providers/shared.rs`
- Modify: `crates/llm-client/tests/oauth_tests.rs`

The `OAuthProvider` trait methods return `std::io::Error` which is inappropriate for OAuth flows. Change to `LlmError`. Also simplify `resolve_oauth_key` signature from `&Option<Arc<dyn OAuthProvider>>` to `Option<&Arc<dyn OAuthProvider>>`.

- [ ] **Step 1: Change OAuthProvider trait error types**

In `oauth.rs`, change the trait:

```rust
use crate::error::LlmError;

#[async_trait::async_trait]
pub trait OAuthProvider: Send + Sync {
    fn provider_name(&self) -> &str;

    /// Acquire a fresh token (e.g. via browser redirect or device code).
    async fn login(&self) -> Result<OAuthToken, LlmError>;

    /// Refresh an existing token.
    async fn refresh(&self, token: &OAuthToken) -> Result<OAuthToken, LlmError>;

    /// Load a previously saved token from disk / keyring / etc.
    fn load_token(&self) -> Option<OAuthToken>;

    /// Persist a token for later reuse.
    fn save_token(&self, token: &OAuthToken) -> Result<(), LlmError>;
}
```

Remove the `use std::io::Error` import if it was being used.

- [ ] **Step 2: Simplify `resolve_oauth_key` signature**

```rust
/// Resolve an API key via OAuth when available.
///
/// 1. If an `OAuthProvider` is configured, try loading the token.
/// 2. If the token is expired, attempt refresh.
/// 3. Return the access token on success.
/// 4. On any failure (missing token, refresh error, etc.), return `None`
///    so the caller can fall back to the next key source.
pub async fn resolve_oauth_key(
    oauth: Option<&std::sync::Arc<dyn OAuthProvider>>,
) -> Option<SecretString> {
    let oauth = oauth?;
    let token = oauth.load_token()?;
    let token = if is_expired(&token) {
        oauth.refresh(&token).await.ok()?
    } else {
        token
    };
    Some(token.access_token)
}
```

- [ ] **Step 3: Update callers**

In the shared.rs macro (Task 9), update the `resolve_api_key` method to pass `self.oauth_provider.as_ref()` instead of `&self.oauth_provider`:

```rust
if let Some(key) =
    crate::oauth::resolve_oauth_key(self.oauth_provider.as_ref()).await
{
    return Ok(key);
}
```

- [ ] **Step 4: Update OAuth tests**

In `tests/oauth_tests.rs`, update any test mock implementations of `OAuthProvider` to return `Result<OAuthToken, LlmError>` instead of `Result<OAuthToken, std::io::Error>`. Change `std::io::Error` imports to `LlmError`.

Run: `cargo test -p llm-client oauth`
Expected: all oauth tests PASS

- [ ] **Step 5: Run full test suite**

Run: `cargo test -p llm-client`
Expected: all ~184 tests PASS

- [ ] **Step 6: Commit**

```bash
git add crates/llm-client/src/oauth.rs crates/llm-client/src/providers/shared.rs crates/llm-client/tests/oauth_tests.rs
git commit -m "refactor: use LlmError in OAuthProvider, simplify resolve_oauth_key signature"
```

---

### Task 12: Add request body validation tests

**Files:** Create: `crates/llm-client/tests/provider_requests.rs`

Add integration tests using wiremock to verify the HTTP request bodies sent to each provider are correctly structured.

- [ ] **Step 1: Create test file**

Create `crates/llm-client/tests/provider_requests.rs`:

```rust
use llm_client::{
    LlmProvider, MistralProvider,
    providers::anthropic::AnthropicProvider,
    providers::google::GoogleProvider,
    providers::openai::OpenAiProvider,
};
use llm_client::types::{Content, LlmContext, Message, ToolDef, UserMessage};
use secrecy::SecretString;
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio_util::sync::CancellationToken;
use wiremock::{Mock, MockServer, ResponseTemplate};
use wiremock::matchers::{method, path, body_string_contains};

/// Verify Anthropic request body structure.
#[tokio::test]
async fn test_anthropic_request_body_structure() {
    let server = MockServer::start().await;
    let body_was_valid = Arc::new(AtomicBool::new(false));
    let b = body_was_valid.clone();

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(move |req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            // Verify required Anthropic fields
            assert_eq!(body["model"], "claude-sonnet-4-20250514");
            assert!(body["max_tokens"].as_u64().unwrap() > 0);
            assert!(body["messages"].is_array());
            assert!(body["messages"][0]["role"] == "user");
            assert!(body["messages"][0]["content"].is_array());

            b.store(true, Ordering::SeqCst);
            // Return a minimal valid SSE stream
            ResponseTemplate::new(200)
                .set_body_string("event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"test\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":0,\"output_tokens\":0}}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n")
        })
        .mount(&server)
        .await;

    let api_key = SecretString::new("test-key".into());
    let provider = AnthropicProvider::with_base_url(Some(api_key), &server.uri());

    let ctx = LlmContext {
        system_prompt: Some("You are a helpful assistant.".into()),
        messages: vec![Message::User(UserMessage {
            content: vec![Content::Text {
                text: "hello".into(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        })],
        tools: Some(vec![ToolDef {
            name: "test_tool".into(),
            description: "A test tool".into(),
            parameters: json!({"type": "object", "properties": {"x": {"type": "string"}}}),
        }]),
    };

    let mut stream = provider
        .stream(
            "claude-sonnet-4-20250514",
            ctx,
            llm_client::StreamOptions::default(),
            CancellationToken::new(),
        )
        .await
        .unwrap();

    // Drain the stream
    while stream.next().await.is_some() {}

    assert!(body_was_valid.load(Ordering::SeqCst));
}

/// Verify OpenAI request body structure.
#[tokio::test]
async fn test_openai_request_body_structure() {
    let server = MockServer::start().await;
    let body_was_valid = Arc::new(AtomicBool::new(false));
    let b = body_was_valid.clone();

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(move |req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            assert_eq!(body["model"], "gpt-5.2");
            assert!(body["messages"].is_array());
            assert!(body["messages"][0]["role"] == "user");
            assert!(body["tools"].is_array());
            b.store(true, Ordering::SeqCst);
            ResponseTemplate::new(200)
                .set_body_string("data: {\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"test\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello\"},\"finish_reason\":null}]}\n\ndata: {\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"test\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n")
        })
        .mount(&server)
        .await;

    let provider = OpenAiProvider::with_base_url(
        Some(SecretString::new("test-key".into())),
        &server.uri(),
    );

    let ctx = LlmContext {
        system_prompt: None,
        messages: vec![Message::User(UserMessage {
            content: vec![Content::Text {
                text: "hello".into(),
                text_signature: None,
            }],
            timestamp: std::time::SystemTime::now(),
        })],
        tools: Some(vec![ToolDef {
            name: "test_tool".into(),
            description: "test".into(),
            parameters: json!({"type": "object"}),
        }]),
    };

    let mut stream = provider
        .stream("gpt-5.2", ctx, llm_client::StreamOptions::default(), CancellationToken::new())
        .await
        .unwrap();
    while stream.next().await.is_some() {}
    assert!(body_was_valid.load(Ordering::SeqCst));
}
```

Only Anthropic and OpenAI are tested with wiremock since Google and Mistral tests would follow the same pattern. Add Google and Mistral variants if time permits.

- [ ] **Step 2: Run the new tests**

Run: `cargo test -p llm-client --test provider_requests`
Expected: 2 tests PASS (or more if Google/Mistral added)

- [ ] **Step 3: Commit**

```bash
git add crates/llm-client/tests/provider_requests.rs
git commit -m "test: add request body validation tests for providers"
```

---

## Verification Checklist

After all tasks complete, run the full verification:

```bash
cargo test -p llm-client
cargo clippy -p llm-client --all-features -- -D warnings
cargo check -p agent-core  # Ensure downstream compiles
cargo check -p extensions   # Ensure downstream compiles
```

Expected: all commands succeed with zero errors and zero warnings.
