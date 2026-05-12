# 修复 models_data.rs 中 compat 字段未接入自动检测逻辑

**Goal:** 将 `models_data.rs` 中所有 model 的硬编码 `compat: ModelCompat::None` 替换为调用 `compat.rs` 中已实现的自动检测逻辑，使 model registry 真正成为 compat 配置的权威来源。

**Architecture:** 修改 `build_models()` 函数，在插入每个 model 时根据 `api` 字段自动调用对应的 detect 函数。`compat.rs` 中的 `detect_openai_compat(provider, base_url, model_id)` 和 `detect_anthropic_compat(provider, base_url)` 已完整实现，只需在 model 构造阶段接入。

**Tech Stack:** Rust, llm-client crate

---

### 问题分析

当前状态：
1. `models_data.rs` 中 `insert!` 宏硬编码 `compat: crate::models::ModelCompat::None`（line 28）
2. `compat.rs` 中 `detect_openai_compat()` 和 `detect_anthropic_compat()` 已实现完整的自动检测逻辑
3. Provider 代码（如 `openai.rs:139`）通过 `get_model()` 读取 `model.compat`，但registry中全是 `None`，只能 fallback 到 `OpenAiCompat::default()`
4. 这导致所有基于 model registry 的兼容性配置检测**实际上未生效**

### 修复方案

修改 `build_models()` 中 model 的 `compat` 字段填充逻辑：
- `api == "openai-completions"` → `ModelCompat::OpenAI(detect_openai_compat(provider, base_url, id))`
- `api == "anthropic-messages"` → `ModelCompat::Anthropic(detect_anthropic_compat(provider, base_url))`
- 其他 → `ModelCompat::None`

---

### Task 1: 修改 models_data.rs 中的 compat 字段填充

**Files:**
- Modify: `crates/llm-client/src/models_data.rs`

- [ ] **Step 1: 修改 `build_models()` 函数末尾，在返回前为每个 model 填充 compat**

  在 `build_models()` 函数末尾、`m` 返回前，添加循环遍历所有 model 并填充 compat：

```rust
    // Fill compat fields using auto-detection logic
    for (key, model) in m.iter_mut() {
        model.compat = match model.api.as_str() {
            "openai-completions" => {
                crate::models::ModelCompat::OpenAI(crate::compat::detect_openai_compat(
                    &model.provider,
                    &model.base_url,
                    &model.id,
                ))
            }
            "anthropic-messages" => {
                crate::models::ModelCompat::Anthropic(crate::compat::detect_anthropic_compat(
                    &model.provider,
                    &model.base_url,
                ))
            }
            _ => crate::models::ModelCompat::None,
        };
    }

    m
```

### Task 2: 验证并运行测试

- [ ] **Step 2: 编译检查**

  Run: `cargo check -p llm-client`
  Expected: 零错误。

- [ ] **Step 3: 运行 llm-client 测试**

  Run: `cargo test -p llm-client -- --nocapture`
  Expected: 全部通过，特别关注 compat 相关测试。

- [ ] **Step 4: Commit**

```bash
git add crates/llm-client/src/models_data.rs
git commit -m "fix(llm-client): wire up compat auto-detection in model registry

Replace hard-coded ModelCompat::None in models_data.rs with
auto-detected compat values from compat::detect_openai_compat()
and compat::detect_anthropic_compat(). Provider code that reads
model.compat from the registry will now receive actual per-model
compatibility flags instead of always falling back to defaults."
```

---

## 变更范围

| 文件 | 变更 | 影响 |
|---|---|---|
| `models_data.rs` | `build_models()` 返回前遍历填充 `compat` | 所有 provider 的 `get_model()` 调用现在能读到实际的 compat 配置 |
| `compat.rs` | 无变更 | 已有逻辑被复用 |
| Provider 代码 | 无变更 | 自动生效：openai/mistral/google/anthropic 的 compat 读取逻辑现在开始工作 |

计划完成。是否立即执行？
