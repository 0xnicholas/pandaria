# Plan: 在 agent-core 中实现 Skills 支持

> 创建日期: 2026-05-14  
> 状态: Review 完成，已优化至 v1.2  
> 对应 Spec: `docs/specs/skills-spec.md`

---

## 1. 目标与范围

在 `agent-core` crate 中实现类似 pi.dev 的 skills 机制：
- Skill 文件（`SKILL.md`，Markdown + YAML frontmatter）的发现与加载
- Skill 索引以 XML 格式注入 system prompt
- `/skill:name` 显式调用，将 skill 内容作为 steer message 注入

**不做**：
- Skill marketplace / 分发平台（PRD 明确 Out of Scope）
- 运行时热重载（v0.1 在 SessionActor 创建时加载一次）
- 多语言 skill SDK

---

## 2. 为什么放在 agent-core

pi.dev 的 `skills.ts` 位于 `packages/coding-agent/src/core/`，与 `system-prompt.ts`、`agent-session.ts` 同级。skills 的本质是**影响 LLM 所见上下文**，属于 agent 核心协议的一部分。

pandaria 的分层架构中，agent-core 负责 "Agent loop、Tool pipeline、Session 生命周期"。skills 的注入发生在 `AgentLoop::run_turn()` 构建 `LlmContext` 的阶段，`/skill:name` 展开发生在 `SessionActor::prompt()` 的输入预处理阶段——两者都在 agent-core 的职责范围内。

若放在 extensions，extensions 依赖 agent-core，无法反向将 skills 数据直接注入 `LlmContext`（只能通过 hook 间接绕过，破坏了 skills 作为一等公民的地位）。

---

## 3. 优化记录

### v1.2 优化（对比 pi.dev 后）

| 优先级 | 优化项 | 说明 |
|---|---|---|
| **P0** | **移除 `Skill.content`，改为按需读取** | 对比 pi.dev 发现其 `Skill` 接口无 `content` 字段，仅保留文件路径。预加载 content 在多租户场景下存在内存隐患（50 skills × 10KB × 1000 tenants = 500MB）。`/skill:name` 展开时现场异步读取。 |
| **P1** | **增加 `LoadSkillsResult` 诊断系统** | 对比 pi.dev 的 `ResourceDiagnostic[]`，将 `load_skills` 返回值从 `Result<Vec<Skill>>` 改为 `LoadSkillsResult { skills, diagnostics }`，部分加载失败不再中断整体流程。 |
| **P1** | **增加 `validate_skill_name`** | 对比 pi.dev 的严格验证规则（匹配父目录、正则 `^[a-z0-9-]+$`、64字符限制），补充 name 验证，防止命名混乱。 |
| **P2** | **增加 `SkillSource` 字段** | 对比 pi.dev 的 `sourceInfo`，记录 skill 来源（User / Project / Path），便于调试和碰撞追踪。 |
| **P2** | **简化 ignore 过滤** | pi.dev 使用完整 `ignore` npm 包；pandaria v0.1 先采用简化实现（逐行读取 `.gitignore` 匹配），v0.2 可升级至 `ignore` crate。 |

### v1.1 修正

| 问题 | 修正 |
|---|---|
| `SessionActor::new()` 中调用 `.await` | `new()` 接收 `skills: Vec<Skill>` 参数，由上层异步加载后传入 |
| Frontmatter 提取逻辑缺失 | 在 `scanner.rs` 中增加 `extract_frontmatter()` 函数 |
| Skills 注入时机 | 在 `LlmContext` 构建时即注入，`on_before_provider_request` hook 能看到完整 system_prompt |
| `parse_skill_invocation` 位置 | 从 `session.rs` 移至 `skills/mod.rs` |
| `escape_xml` 缺失 | 在 `injector.rs` 中补充实现 |

---

## 4. 模块设计

### 4.1 新增 `agent-core/src/skills/`

```
src/skills/
├── mod.rs       # 模块入口、parse_skill_invocation、导出
├── types.rs     # Skill、SkillFrontmatter、SkillSource、SkillDiagnostic 类型
├── scanner.rs   # 目录扫描、SKILL.md 发现、碰撞检测、frontmatter 提取、name 验证
├── loader.rs    # SkillLoader trait + FileSystemSkillLoader 默认实现
└── injector.rs  # format_skills_for_prompt() + escape_xml()
```

#### `types.rs`

```rust
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct SkillFrontmatter {
    pub name: Option<String>,
    pub description: String,
    #[serde(rename = "disable-model-invocation", default)]
    pub disable_model_invocation: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillSource {
    User,
    Project,
    Path,
}

#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub file_path: String,
    pub base_dir: String,
    pub source: SkillSource,
    pub disable_model_invocation: bool,
    // 注意：无 content 字段，按需读取
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillDiagnosticKind {
    Warning,
    Collision,
}

#[derive(Debug, Clone)]
pub struct SkillDiagnostic {
    pub path: String,
    pub kind: SkillDiagnosticKind,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct LoadSkillsResult {
    pub skills: Vec<Skill>,
    pub diagnostics: Vec<SkillDiagnostic>,
}
```

#### `scanner.rs`

- 扫描路径（可配置）：
  - 用户级：`~/.agents/skills/`
  - 项目级：`<cwd>/.agents/skills/`
  - 显式路径：通过配置传入
- 发现规则（同 pi.dev）：
  - 目录含 `SKILL.md` → 视为 skill root，停止递归
  - 否则加载根目录下的直接 `.md` 子文件
  - 递归子目录继续查找 `SKILL.md`
- 支持 `.gitignore` / `.ignore` 过滤（简化实现：逐行读取并匹配）
- 碰撞检测：同名 skill 先加载的胜出，后加载的报 `Collision` 诊断
- 路径安全：禁止访问 `/workspace/{tenant_id}/` 以外的路径（符合 AGENTS.md 安全约束）

**Name 验证**（新增，对齐 pi.dev）：

```rust
const MAX_NAME_LENGTH: usize = 64;

fn validate_skill_name(name: &str, parent_dir_name: &str) -> Vec<String> {
    let mut errors = Vec::new();
    if name != parent_dir_name {
        errors.push(format!(r#"name "{}" does not match parent directory "{}""#, name, parent_dir_name));
    }
    if name.len() > MAX_NAME_LENGTH {
        errors.push(format!("name exceeds {} characters ({})", MAX_NAME_LENGTH, name.len()));
    }
    if !regex::Regex::new(r"^[a-z0-9-]+$").unwrap().is_match(name) {
        errors.push("name contains invalid characters (must be lowercase a-z, 0-9, hyphens only)".to_string());
    }
    if name.starts_with('-') || name.ends_with('-') {
        errors.push("name must not start or end with a hyphen".to_string());
    }
    if name.contains("--") {
        errors.push("name must not contain consecutive hyphens".to_string());
    }
    errors
}
```

**Frontmatter 提取**：

```rust
fn extract_frontmatter(content: &str) -> Option<(String, String)> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }
    let after_first = &trimmed[3..];
    let rest = after_first.trim_start_matches('-').trim_start();
    let end_pos = rest.find("\n---")?;
    let yaml = rest[..end_pos].trim().to_string();
    let body = rest[end_pos + 4..].trim_start().to_string();
    Some((yaml, body))
}
```

#### `loader.rs`

```rust
use async_trait::async_trait;

#[async_trait]
pub trait SkillLoader: Send + Sync {
    async fn load_skills(&self) -> LoadSkillsResult;
}

/// 默认实现：从文件系统扫描
pub struct FileSystemSkillLoader {
    pub user_skills_dir: String,
    pub project_skills_dir: String,
    pub explicit_paths: Vec<String>,
}

#[async_trait]
impl SkillLoader for FileSystemSkillLoader {
    async fn load_skills(&self) -> LoadSkillsResult {
        // 用 tokio::task::spawn_blocking 包装同步文件扫描
    }
}
```

**关键设计**：`load_skills` 返回 `LoadSkillsResult`（非 `Result`），所有错误以 `SkillDiagnostic` 形式返回，不中断整体加载。与 pi.dev 的 `LoadSkillsResult { skills, diagnostics }` 对齐。

#### `injector.rs`

```rust
pub fn format_skills_for_prompt(skills: &[Skill]) -> String {
    let visible: Vec<_> = skills.iter().filter(|s| !s.disable_model_invocation).collect();
    if visible.is_empty() {
        return String::new();
    }

    let mut lines = vec![
        "\n\nThe following skills provide specialized instructions for specific tasks.".to_string(),
        "Use the read tool to load a skill's file when the task matches its description.".to_string(),
        "When a skill file references a relative path, resolve it against the skill directory.".to_string(),
        String::new(),
        "<available_skills>".to_string(),
    ];

    for skill in visible {
        lines.push("  <skill>".to_string());
        lines.push(format!("    <name>{}</name>", escape_xml(&skill.name)));
        lines.push(format!("    <description>{}</description>", escape_xml(&skill.description)));
        lines.push(format!("    <location>{}</location>", escape_xml(&skill.file_path)));
        lines.push("  </skill>".to_string());
    }

    lines.push("</available_skills>".to_string());
    lines.join("\n")
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
     .replace('<', "&lt;")
     .replace('>', "&gt;")
     .replace('"', "&quot;")
     .replace('\'', "&apos;")
}
```

#### `mod.rs`

```rust
pub mod types;
pub mod scanner;
pub mod loader;
pub mod injector;

pub use types::*;
pub use loader::*;
pub use injector::*;

/// 解析 `/skill:name` 调用语法。
/// 返回 skill name（不含 `/skill:` 前缀）。
pub fn parse_skill_invocation(text: &str) -> Option<&str> {
    text.strip_prefix("/skill:")
}
```

### 4.2 修改现有模块

#### `agent-core/src/harness/agent_loop.rs`

`AgentLoopConfig` 新增 `skills` 字段：

```rust
pub struct AgentLoopConfig {
    // ... existing fields ...
    pub skills: Vec<crate::skills::Skill>,
}
```

在 `run_turn()` 中，**构建 `LlmContext` 时即注入 skills**（使 `on_before_provider_request` hook 能看到）：

```rust
let skills_xml = crate::skills::format_skills_for_prompt(&self.config.skills);
let effective_system_prompt = system_prompt.as_ref().map(|sp| {
    if skills_xml.is_empty() { sp.clone() } else { format!("{}\n{}", sp, skills_xml) }
}).or_else(|| {
    if skills_xml.is_empty() { None } else { Some(skills_xml) }
});

let mut stream_opts = self.config.stream_options.clone();
let mut ctx = LlmContext {
    system_prompt: effective_system_prompt,
    messages: transformed,
    tools: build_tool_defs(&self.config.tools),
};
```

#### `agent-core/src/harness/session.rs`

`SessionActor` 新增 `skills` 字段：

```rust
skills: Vec<Skill>,
```

`new()` 新增 `skills: Vec<Skill>` 参数（第 10 个参数，已有 `#[allow(clippy::too_many_arguments)]`）：

```rust
#[allow(clippy::too_many_arguments)]
pub fn new(
    tenant_id: String,
    session_id: String,
    system_prompt: String,
    model: String,
    provider: Arc<dyn ai_provider::LlmProvider>,
    hook_dispatcher: Arc<dyn HookDispatcher>,
    compaction_actor: Arc<CompactionActor>,
    tools: Vec<AgentToolRef>,
    store: Option<Arc<dyn SessionStore>>,
    skills: Vec<Skill>,  // 新增
) -> Self {
    // ... existing ...
    let mut actor = Self {
        // ...
        skills,  // 新增
        // ...
    };
    // ...
}
```

`run_with_messages()` 中构建 `AgentLoopConfig` 时传入 `skills`：

```rust
let config = AgentLoopConfig {
    // ...
    skills: self.skills.clone(),  // 新增
};
```

`prompt()` 中处理 `/skill:name` 调用（**关键变更：现场异步读取 content**）：

```rust
pub async fn prompt(&mut self, text: String) -> Result<Vec<AgentMessage>, AgentError> {
    // 解析 /skill:name 语法
    if let Some(skill_name) = crate::skills::parse_skill_invocation(&text) {
        if let Some(skill) = self.skills.iter().find(|s| s.name == skill_name) {
            // 现场异步读取 skill 内容（对比 pi.dev 的惰性加载）
            let content = tokio::fs::read_to_string(&skill.file_path).await
                .map_err(|e| AgentError::SkillLoadFailed(
                    format!("failed to read skill {}: {}", skill.name, e)
                ))?;

            let skill_msg = AgentMessage::User(ai_provider::UserMessage {
                content: vec![Content::Text {
                    text: format!("[Skill: {}]\n{}", skill.name, content),
                    text_signature: None,
                }],
                timestamp: std::time::SystemTime::now(),
            });
            self.steer(skill_msg);
            return self.run_with_messages(None).await;
        } else {
            return Err(AgentError::SkillNotFound(skill_name.to_string()));
        }
    }

    // 正常流程
    let user_msg = AgentMessage::User(ai_provider::UserMessage {
        content: vec![Content::Text { text, text_signature: None }],
        timestamp: std::time::SystemTime::now(),
    });
    self.push_message(user_msg);
    self.run_with_messages(None).await
}
```

#### `agent-core/src/error.rs`

新增错误变体：

```rust
#[derive(Debug, Clone, Error)]
pub enum AgentError {
    // ... existing variants ...

    #[error("skill not found: {0}")]
    SkillNotFound(String),

    #[error("skill load failed: {0}")]
    SkillLoadFailed(String),
}
```

#### `agent-core/src/lib.rs`

新增模块导出：

```rust
pub mod skills;
```

并更新兼容性 re-export：

```rust
pub use skills::{
    Skill, SkillFrontmatter, SkillSource, SkillDiagnostic, SkillDiagnosticKind,
    LoadSkillsResult, SkillLoader, FileSystemSkillLoader,
    format_skills_for_prompt, parse_skill_invocation,
};
```

### 4.3 Cargo.toml 依赖

**Workspace 根 `Cargo.toml`** 新增：

```toml
[workspace.dependencies]
# 新增
serde_yaml = "0.9"
```

**`crates/agent-core/Cargo.toml`** 新增：

```toml
[dependencies]
# 新增
serde_yaml = { workspace = true }
regex = { workspace = true }  # name 验证用

[dev-dependencies]
# 新增（仅测试用）
tempfile = "3"
```

> `walkdir` 省略：用 `std::fs::read_dir` 手动递归足够，减少依赖。

---

## 5. 与 Extension 的集成

Extension 可通过两种方式参与 skills：

1. **贡献 skill paths**（v0.2）：新增 `on_resources_discover` hook，Extension 返回额外 skill 目录，由 `SessionActor` 汇总后传给 `SkillLoader`
2. **自定义 SkillLoader**（v0.1）：上层构造 `Arc<dyn SkillLoader>` 注入 `SessionActor`，完全控制 skill 来源

v0.1 采用方案 2，保持简单。

**上层调用示例**（future tenant/api-gateway 层）：

```rust
let loader = FileSystemSkillLoader {
    user_skills_dir: "~/.agents/skills".to_string(),
    project_skills_dir: ".agents/skills".to_string(),
    explicit_paths: vec![],
};
let result = loader.load_skills().await;
for diag in &result.diagnostics {
    tracing::warn!("skill diagnostic: {:?}", diag);
}

let session = SessionActor::new(
    tenant_id, session_id, system_prompt, model,
    provider, hook_dispatcher, compaction_actor,
    tools, store, result.skills,  // skills 作为第 10 个参数传入
);
```

---

## 6. 测试策略

| 测试类型 | 内容 | 位置 |
|---|---|---|
| 单元测试 | `extract_frontmatter`：正常提取、缺失 frontmatter、空 YAML、多行 body | `src/skills/scanner.rs` `#[cfg(test)]` |
| 单元测试 | `validate_skill_name`：合法名、非法字符、过长、不匹配目录、连字符边界 | `src/skills/scanner.rs` `#[cfg(test)]` |
| 单元测试 | `scanner`：目录扫描、SKILL.md 发现、递归边界、碰撞检测、ignore 过滤 | `src/skills/scanner.rs` `#[cfg(test)]` |
| 单元测试 | `injector`：XML 格式化输出、空 skills、disable_model_invocation 过滤、XML 转义 | `src/skills/injector.rs` `#[cfg(test)]` |
| 单元测试 | `parse_skill_invocation`：匹配 `/skill:name`、不匹配普通文本、空 name | `src/skills/mod.rs` `#[cfg(test)]` |
| 单元测试 | `FileSystemSkillLoader`：部分加载失败时 diagnostics 收集 | `src/skills/loader.rs` `#[cfg(test)]` |
| 集成测试 | `/skill:name` 展开：验证 steer message 正确注入 session entries | `tests/skills_integration_tests.rs` |
| 集成测试 | prompt 注入：验证 LLM context 的 system_prompt 包含 skills XML | `tests/skills_integration_tests.rs` |

测试使用临时目录（`tempfile` crate）创建 mock skills 文件，避免依赖真实文件系统。

---

## 7. 边界与约束

- **安全**：skill 文件路径校验，禁止访问 `/workspace/{tenant_id}/` 以外的路径（符合 AGENTS.md 安全约束）
- **错误处理**：skill 加载失败（文件损坏、frontmatter 缺失、name 非法）不中断 session 启动，报 `SkillDiagnostic` 并跳过该 skill
- **并发**：`SkillLoader::load_skills` 在 `SessionActor::new()` **之前**由调用方执行，结果通过 `Vec<Skill>` 传入。`SessionActor` 内部不执行文件 I/O。
- **内存**：`Skill` 类型**不缓存 content**，仅保留 metadata（name、description、file_path 等）。`/skill:name` 展开时现场 `tokio::fs::read_to_string()`。
- **tenant 隔离**：未来 tenant 层可为不同租户配置不同的 `SkillLoader`，agent-core 无感知

---

## 8. 工作量与风险

| 模块 | 预估行数 | 风险 |
|---|---|---|
| `skills/mod.rs` | 30 | 低 |
| `skills/types.rs` | 50 | 低 |
| `skills/scanner.rs` | 220 | 中（目录扫描边界条件 + frontmatter 提取 + name 验证）|
| `skills/loader.rs` | 100 | 低 |
| `skills/injector.rs` | 60 | 低 |
| `error.rs` 修改 | 10 | 低 |
| `agent_loop.rs` 修改 | 15 | 低 |
| `session.rs` 修改 | 60 | 低 |
| `lib.rs` 修改 | 15 | 低 |
| Cargo.toml 修改 | 15 | 低 |
| 测试 | 320 | 中 |
| **总计** | **~895** | **低-中** |

**主要风险**：
1. `serde_yaml` 对 malformed YAML 的解析行为（已通过 `SkillDiagnostic` 降级处理）
2. `tokio::fs::read_to_string` 在 `/skill:name` 展开时的权限问题（受 AGENTS.md 路径安全约束限制）

---

## 9. 实施步骤

1. **Phase 1**: `skills/mod.rs` + `skills/types.rs` + `skills/injector.rs` + `error.rs` 修改 + 单元测试
2. **Phase 2**: `skills/scanner.rs` + `skills/loader.rs` + 单元测试（含诊断系统、name 验证）
3. **Phase 3**: 修改 `agent_loop.rs` 注入 skills XML + 集成测试
4. **Phase 4**: 修改 `session.rs` 支持 `/skill:name`（现场异步读取）+ 集成测试
5. **Phase 5**: 更新 `README.md` 和 `AGENTS.md`，说明 skills 支持
