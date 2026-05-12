# 修复 content_filter 的 .unwrap() 和正则编译性能

**Goal:** 消除 `extensions/src/builtins/content_filter.rs` 中 6 处 `.unwrap()`，并将 PII 正则编译从运行时每次调用改为构造期一次性预编译。

**Architecture:** 扩展现有的 `regex_cache` 机制。`regex_cache: Vec<Option<Arc<Regex>>>` 当前仅在 `FilterRule::Regex` 规则上预编译；将其扩展为 `FilterRule::PII` 规则也在构造期预编译，后续 `redact_text` / `check_text` 直接从缓存读取。

**Tech Stack:** Rust, regex, extensions crate

---

### Task 1: 扩展 regex_cache 以覆盖 PII 规则

**Files:**
- Modify: `crates/extensions/src/builtins/content_filter.rs:62-74`

- [ ] **Step 1: 修改 `new()` 方法，为 PII 规则预编译正则**

  将 `new()` 中的 match 扩展为也为 `FilterRule::PII` 分支编译正则：

```rust
for (rule, _) in &rules {
    let re = match rule {
        FilterRule::Regex(pattern) => {
            Some(Arc::new(Regex::new(pattern).expect("invalid regex in content filter")))
        }
        FilterRule::PII(pii_type) => {
            let pattern = match pii_type {
                PIIType::Email => r"[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}",
                PIIType::Phone => r"\b\d{3}[-.]?\d{3}[-.]?\d{4}\b",
                PIIType::CreditCard => r"\b(?:\d[ -]*?){13,16}\b",
            };
            Some(Arc::new(Regex::new(pattern).expect("invalid PII regex in content filter")))
        }
        _ => None,
    };
    regex_cache.push(re);
}
```

### Task 2: 消除 redact_text 中的 .unwrap() 和重复编译

**Files:**
- Modify: `crates/extensions/src/builtins/content_filter.rs:112-128`

- [ ] **Step 2: 重写 `FilterRule::PII` 分支，从 regex_cache 读取**

```rust
FilterRule::PII(pii_type) => {
    let replacement = match pii_type {
        PIIType::Email => "[REDACTED_EMAIL]",
        PIIType::Phone => "[REDACTED_PHONE]",
        PIIType::CreditCard => "[REDACTED_CREDIT_CARD]",
    };
    if let Some(re) = self.regex_cache.get(idx).and_then(|o| o.as_ref()) {
        result = re.replace_all(&result, replacement).to_string();
    }
}
```

### Task 3: 消除 check_text 中的 .unwrap() 和重复编译

**Files:**
- Modify: `crates/extensions/src/builtins/content_filter.rs:152-158`

- [ ] **Step 3: 重写 `FilterRule::PII` 分支，从 regex_cache 读取**

```rust
FilterRule::PII(_) => {
    self.regex_cache
        .get(idx)
        .and_then(|o| o.as_ref())
        .and_then(|re| re.find(text))
        .map(|m| m.as_str().to_string())
}
```

### Task 4: 验证

- [ ] **Step 4: 运行现有测试**

Run: `cargo test -p extensions content_filter -- --nocapture`
Expected: 全部 6 个测试通过。

- [ ] **Step 5: 编译检查**

Run: `cargo check -p extensions`
Expected: 零错误、零警告。

- [ ] **Step 6: Commit**

```bash
git add crates/extensions/src/builtins/content_filter.rs
git commit -m "fix(extensions): eliminate .unwrap() and cache PII regexes in content_filter

- Pre-compile PII regexes (Email, Phone, CreditCard) during
  ContentFilterExtension construction, alongside existing Regex
  rule caching.
- Remove 6 .unwrap() calls from redact_text() and check_text().
- PII regexes are now read from regex_cache at runtime instead
  of re-compiled on every call."
```

---

## 变更概要

| 问题 | 修复前 | 修复后 |
|---|---|---|
| `.unwrap()` | 6 处（PII 分支） | 0 处 |
| PII 正则编译 | 每次 `redact_text` / `check_text` 调用时现场编译 | 构造期一次性预编译，存入 `regex_cache` |
| `FilterRule::Regex` | 已有缓存 | 不变 |
| `regex_cache` 语义 | 仅缓存 `Regex` 规则 | 扩展为也缓存 `PII` 规则 |

计划完成。是否立即执行？
