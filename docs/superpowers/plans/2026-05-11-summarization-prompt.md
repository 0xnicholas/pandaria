# SUMMARIZATION_SYSTEM_PROMPT 填充实施计划

**Goal:** 将 `agent-core/src/compaction.rs:419` 的占位符 `SUMMARIZATION_SYSTEM_PROMPT` 替换为生产级 system prompt，确保压缩后的摘要质量足以支撑后续对话恢复。

**Architecture:** 单文件常量替换，零架构变更。Prompt 设计遵循用户确认的 4 条约束：跟随对话语言、代码保留阈值（≤15 行核心代码保留完整，超过则保留签名+一句话描述）、不摘要 thinking 块、增量更新语义（通过 user prompt 层面保证，system prompt 只需定义摘要者的角色和边界）。

**Tech Stack:** Rust, agent-core

---

### Task 1: 替换 SUMMARIZATION_SYSTEM_PROMPT

**Files:**
- Modify: `crates/agent-core/src/compaction.rs:419`

- [ ] **Step 1: 替换常量内容**

  将第 419 行的占位符替换为以下完整 prompt：

```rust
const SUMMARIZATION_SYSTEM_PROMPT: &str = r#"You are a technical conversation summarizer for a software engineering AI assistant. Your summaries will be injected as context for future conversation turns, so they must be self-contained.

RULES:
1. LANGUAGE: Use the same language as the conversation being summarized.
2. STRUCTURED FORMAT: Always produce summaries in the structured format requested by the user (Overview, Progress, Key Decisions, Current State, Next Steps, Important Files/Functions).
3. PRESERVE EXACT IDENTIFIERS: File paths, function names, class names, variable names, configuration keys, URLs, commit hashes, and error messages must be preserved verbatim. Do not paraphrase technical identifiers.
4. CODE SNIPPETS:
   - If a core code block is 15 lines or fewer, preserve it in full.
   - If it exceeds 15 lines, keep only the function/class signature and describe the logic in one sentence.
5. IGNORE THINKING BLOCKS: Do not include the assistant's internal reasoning or thinking blocks in the summary. Summarize only the final outputs, decisions, and actions.
6. CONCISE BUT COMPLETE: Remove conversational filler and pleasantries. Retain every technical fact, decision, and action item. The summary must contain enough information for the assistant to continue work without asking clarifying questions about prior context.
7. CAUSALITY: Preserve the logical chain of what was attempted, what failed or succeeded, and why specific decisions were made.
8. NO META-COMMENTARY: Do not add phrases like "Here is the summary" or "In summary". Output only the structured content."#;
```

- [ ] **Step 2: 验证编译**

  Run: `cargo check -p agent-core`
  Expected: 零错误、零警告通过。

- [ ] **Step 3: Commit**

```bash
git add crates/agent-core/src/compaction.rs
git commit -m "feat(agent-core): replace placeholder SUMMARIZATION_SYSTEM_PROMPT

Replace the stub system prompt with a production-grade instruction
that enforces: language parity, exact identifier preservation,
15-line code threshold, thinking-block exclusion, and causal
chain retention. This directly impacts compaction quality and
context recovery across session turns."
```

---

## 变更范围确认

| 检查项 | 状态 |
|---|---|
| 仅修改常量，零 API 变更 | 是 |
| 调用点（line 500, 576）无需改动 | 是 |
| 无需新增测试（prompt 质量通过集成测试在更高层面覆盖） | 是 |
| docs/specs 中的占位符（line 1434）属于历史文档，非代码，不修改 | 是 |

计划完成。由于变更极小且无风险，推荐直接 inline 执行（单步修改 + cargo check）。是否立即执行？
