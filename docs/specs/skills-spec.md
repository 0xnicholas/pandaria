# Skills 技术规格文档

> 版本: 1.0  
> 状态: Draft  
> 对应 Plan: `docs/plans/skills-support-plan.md`

---

## 1. 术语定义

| 术语 | 定义 |
|---|---|
| **Skill** | 一组结构化指令，以 Markdown 文件形式存在，用于指导 LLM 完成特定任务 |
| **SKILL.md** | Skill 的标准文件名，包含 YAML frontmatter + Markdown body |
| **Frontmatter** | SKILL.md 文件顶部 `---` 包裹的 YAML 元数据段 |
| **Skill Index** | 注入 system prompt 的 XML 块，列出当前 session 可用的 skills |
| **Skill Invocation** | 用户通过 `/skill:name` 显式调用某个 skill |
| **SkillLoader** | 负责从外部来源（文件系统、数据库等）发现并加载 skills 的 trait |
| **SkillDiagnostic** | Skill 加载过程中的诊断信息（warning / collision）|

---

## 2. Skill 文件格式规范

### 2.1 文件名

- 标准文件名：`SKILL.md`
- 备用：目录根目录下的任意 `.md` 文件（当目录无 `SKILL.md` 时）

### 2.2 文件结构

```markdown
---
name: skill-name
description: A concise description of what this skill does.
disable-model-invocation: false
---

# Skill Title

Detailed instructions for the LLM...
```

### 2.3 Frontmatter 字段

| 字段 | 类型 | 必填 | 约束 |
|---|---|---|---|
| `name` | `string` | 否 | 默认使用父目录名；若指定，必须与父目录名一致；`^[a-z0-9-]+$`；最长 64 字符 |
| `description` | `string` | **是** | 非空；最长 1024 字符 |
| `disable-model-invocation` | `boolean` | 否 | 默认 `false`；为 `true` 时 skill 不注入 prompt，仅支持显式调用 |

### 2.4 Name 验证规则

```
1. 若 frontmatter 指定 name，必须与父目录名一致
2. 只能包含小写字母 a-z、数字 0-9、连字符 -
3. 最长 64 字符
4. 不能以连字符开头或结尾
5. 不能包含连续连字符（--）
```

违反以上规则的 skill 会被跳过，并生成 `SkillDiagnostic::Warning`。

### 2.5 内容规范

- frontmatter 与 body 之间用 `---` 分隔（行首）
- body 为 Markdown 格式，对 LLM 可见
- body 中引用的相对路径，解析时应以 skill 所在目录为基准

---

## 3. 数据模型

### 3.1 `Skill`

```rust
pub struct Skill {
    /// Skill 标识名（小写、数字、连字符）
    pub name: String,
    /// 一句话描述，注入 prompt 时使用
    pub description: String,
    /// SKILL.md 的绝对路径
    pub file_path: String,
    /// Skill 所在目录的绝对路径
    pub base_dir: String,
    /// Skill 来源（User / Project / Path）
    pub source: SkillSource,
    /// 是否禁止自动注入 prompt
    pub disable_model_invocation: bool,
}
```

**注意**：`Skill` **不缓存 content**。Skill 内容在 `/skill:name` 调用时现场读取，或在 LLM 自主 read 时通过文件系统访问。

### 3.2 `SkillFrontmatter`

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct SkillFrontmatter {
    pub name: Option<String>,
    pub description: String,
    #[serde(rename = "disable-model-invocation", default)]
    pub disable_model_invocation: bool,
}
```

### 3.3 `SkillSource`

```rust
pub enum SkillSource {
    /// 用户级 skills 目录：~/.agents/skills/
    User,
    /// 项目级 skills 目录：<cwd>/.agents/skills/
    Project,
    /// 显式指定的路径
    Path,
}
```

### 3.4 `SkillDiagnostic`

```rust
pub enum SkillDiagnosticKind {
    Warning,
    Collision,
}

pub struct SkillDiagnostic {
    /// 相关文件路径
    pub path: String,
    /// 诊断类型
    pub kind: SkillDiagnosticKind,
    /// 人类可读的消息
    pub message: String,
}
```

### 3.5 `LoadSkillsResult`

```rust
pub struct LoadSkillsResult {
    /// 成功加载的 skills
    pub skills: Vec<Skill>,
    /// 加载过程中的诊断信息
    pub diagnostics: Vec<SkillDiagnostic>,
}
```

---

## 4. Skill 发现与加载规范

### 4.1 来源优先级

Skill 从以下三个来源加载，按顺序处理：

1. **用户级**：`~/.agents/skills/`
2. **项目级**：`<cwd>/.agents/skills/`
3. **显式路径**：通过配置传入的目录或文件

同名 skill 以**先加载的为准**，后加载的生成 `Collision` 诊断并丢弃。

### 4.2 目录扫描规则

```
scan_dir(dir):
    if dir/SKILL.md exists:
        load_skill(dir/SKILL.md)
        return  // 停止递归

    for entry in dir:
        if entry is .md file:
            load_skill(entry)
        if entry is subdirectory and not hidden:
            scan_dir(entry)  // 递归
```

### 4.3 Ignore 过滤

扫描时读取目录下的 `.gitignore`、`.ignore`、`.fdignore` 文件，排除匹配的文件和目录。

**v0.1 简化实现**：逐行读取 ignore 文件，对每行进行前缀匹配。不支持复杂 glob 语法。

**v0.2 升级路径**：替换为 `ignore` crate，完整支持 gitignore 语义。

### 4.4 加载错误处理

| 错误场景 | 行为 | 诊断 |
|---|---|---|
| 目录不存在 | 跳过，无诊断 | — |
| 文件读取失败 | 跳过该 skill | `Warning` |
| frontmatter 格式错误 | 跳过该 skill | `Warning` |
| `description` 缺失 | 跳过该 skill | `Warning` |
| `name` 验证失败 | 跳过该 skill | `Warning` |
| 同名 skill 碰撞 | 保留先加载的，丢弃后加载的 | `Collision` |

---

## 5. SkillLoader Trait 规范

```rust
#[async_trait]
pub trait SkillLoader: Send + Sync {
    /// 加载所有可用 skills。
    ///
    /// 返回 `LoadSkillsResult`，其中：
    /// - `skills`：成功加载的 skills（可能为空）
    /// - `diagnostics`：加载过程中的所有诊断信息（warning / collision）
    ///
    /// 此方法**不**返回 `Err`，所有错误以诊断形式体现。
    async fn load_skills(&self) -> LoadSkillsResult;
}
```

### 5.1 `FileSystemSkillLoader`

默认实现，从文件系统扫描：

```rust
pub struct FileSystemSkillLoader {
    pub user_skills_dir: String,
    pub project_skills_dir: String,
    pub explicit_paths: Vec<String>,
}
```

**线程安全**：文件扫描是同步 I/O，应在 `tokio::task::spawn_blocking` 中执行。

**路径安全**：所有解析出的绝对路径必须经过校验，禁止访问 `/workspace/{tenant_id}/` 以外的路径。

---

## 6. System Prompt 注入规范

### 6.1 注入时机

在 `AgentLoop::run_turn()` 中，构建 `LlmContext` 时即注入 skills XML。注入位置在 `on_context` hook 之后、`on_before_provider_request` hook 之前。

顺序：
```
on_context (修改 messages)
  → resolve_orphan_tool_calls
  → 构建 LlmContext（system_prompt 已含 skills XML）
  → on_before_provider_request (hook 能看到完整 system_prompt)
  → call_llm_with_retry
```

### 6.2 注入格式

```xml

The following skills provide specialized instructions for specific tasks.
Use the read tool to load a skill's file when the task matches its description.
When a skill file references a relative path, resolve it against the skill directory.

<available_skills>
  <skill>
    <name>{escaped_name}</name>
    <description>{escaped_description}</description>
    <location>{escaped_file_path}</location>
  </skill>
  ...
</available_skills>
```

### 6.3 过滤规则

- `disable_model_invocation = true` 的 skill **不**出现在 `<available_skills>` 中
- 若所有 skill 都被过滤，**不**注入任何 skills 文本（避免空 XML 块）

### 6.4 XML 转义规则

以下字符必须转义：

| 原始字符 | 转义后 |
|---|---|
| `&` | `&amp;` |
| `<` | `&lt;` |
| `>` | `&gt;` |
| `"` | `&quot;` |
| `'` | `&apos;` |

---

## 7. `/skill:name` 调用规范

### 7.1 语法

```
/skill:{name}
```

- 必须以 `/skill:` 开头
- `name` 为 skill 的标识名（小写、数字、连字符）
- 整行输入必须为 `/skill:name`（暂不支持参数，v0.2 可扩展）

### 7.2 解析函数

```rust
pub fn parse_skill_invocation(text: &str) -> Option<&str> {
    text.strip_prefix("/skill:")
}
```

### 7.3 执行流程

```
SessionActor::prompt("/skill:code-review")
  → parse_skill_invocation("/skill:code-review") → Some("code-review")
  → 在 self.skills 中查找 name = "code-review"
    → 未找到：返回 Err(AgentError::SkillNotFound("code-review"))
    → 找到：tokio::fs::read_to_string(&skill.file_path).await
      → 读取失败：返回 Err(AgentError::SkillLoadFailed("..."))
      → 读取成功：构建 steer message 注入队列
        → "[Skill: code-review]\n{content}"
        → self.steer(skill_msg)
        → 执行 self.run_with_messages(None)
```

### 7.4 Steer Message 格式

```
[Skill: {name}]
{skill_file_content}
```

作为 `AgentMessage::User` 注入 steer queue，在下一个 LLM 调用前进入上下文。

---

## 8. 错误处理规范

### 8.1 AgentError 新增变体

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

### 8.2 错误传播矩阵

| 场景 | 错误类型 | 传播行为 |
|---|---|---|
| `/skill:name` 的 name 不存在 | `SkillNotFound` | 返回给调用方（上层可展示给用户）|
| `/skill:name` 文件读取失败 | `SkillLoadFailed` | 返回给调用方 |
| SkillLoader 扫描时文件损坏 | `SkillDiagnostic::Warning` | 不传播，仅记录诊断日志 |
| SkillLoader 同名碰撞 | `SkillDiagnostic::Collision` | 不传播，保留先加载的 skill |
| Skill 注入 prompt 时 XML 生成失败 | 不应发生（纯字符串操作）| — |

---

## 9. 安全约束

### 9.1 路径隔离

- Skill 文件路径必须限制在租户工作区内
- 禁止读取 `/workspace/{tenant_id}/` 以外的路径（与 AGENTS.md Extension 路径约束一致）
- `FileSystemSkillLoader` 在解析路径后必须进行前缀校验

### 9.2 输入校验

- `name` 严格验证（见 2.4），防止路径遍历或注入攻击
- `description` 长度限制 1024 字符，防止超大 frontmatter

### 9.3 资源限制

- 单个 skill 文件大小上限：建议 100KB（可在 loader 中配置）
- 单次加载 skill 数量上限：建议 256 个（防止恶意目录填充）

---

## 10. 与 pi.dev 的兼容性

### 10.1 文件格式兼容

pandaria 的 `SKILL.md` 格式与 pi.dev 完全兼容：
- 相同的 frontmatter 字段（`name`、`description`、`disable-model-invocation`）
- 相同的 Markdown body 结构
- 相同的目录发现规则（SKILL.md 优先、递归子目录）

**注意**：pi.dev 使用 `name` 可选（默认父目录名），pandaria 同样支持。

### 10.2 Prompt 注入兼容

`format_skills_for_prompt` 生成的 XML 格式与 pi.dev 完全一致：
- 相同的 `<available_skills>` 结构
- 相同的 `<name>` / `<description>` / `<location>` 字段
- 相同的引导语（"Use the read tool to load..."）

这意味着：为 pi.dev 编写的 skill，LLM 在 pandaria 中以相同方式理解和使用。

### 10.3 不兼容点

| 功能 | pi.dev | pandaria |
|---|---|---|
| Skill 内容预加载 | `Skill` 接口无 `content` | `Skill` 同样无 `content`（v1.2 已修正）✅ 兼容 |
| 诊断系统 | `ResourceDiagnostic[]` | `SkillDiagnostic[]`（概念等价，类型不同）|
| SourceInfo | `sourceInfo: SourceInfo` | `source: SkillSource`（简化版）|
| 热重载 | `/reload` 支持 | 不支持 |
| Marketplace | npm/git 包分发 | 不支持 |
| Extension 贡献 paths | `resources_discover` 事件 | v0.1 通过 `SkillLoader` 注入，v0.2 可能加 hook |

---

## 11. 版本演进路线图

| 版本 | 功能 | 说明 |
|---|---|---|
| **v0.1** | 文件系统加载、prompt 注入、`/skill:name`、诊断系统 | 当前 spec 范围 |
| **v0.2** | `ignore` crate、Extension `resources_discover` hook、skill 参数 | 升级 ignore 过滤；Extension 可贡献 skill paths |
| **v0.3** | 运行时热重载、skill 缓存层 | 支持 `/reload` 式重新加载；Redis 缓存 skill 索引 |

---

## 12. 附录：示例 Skill 文件

```markdown
---
name: rust-debug
description: Debug Rust async issues using tokio tracing and structured logging.
---

# Rust Async Debugging

When debugging Rust async code:

1. Check for `tokio::spawn` tasks that may have panicked silently
2. Use `tracing::instrument` spans to track async boundaries
3. Verify `CancellationToken` propagation through the task tree
4. Look for `blocking_send` or `blocking_recv` in async contexts

## Common Patterns

- `tokio::select!` bias: check if one branch starves others
- `Semaphore` leaks: ensure permits are always released (use `Drop` guard)
```

```markdown
---
name: security-review
description: Perform a security-focused code review looking for injection risks, auth bypasses, and secret leaks.
disable-model-invocation: true
---

# Security Review Checklist

- [ ] SQL injection risks
- [ ] Command injection in shell exec
- [ ] Hardcoded secrets / API keys
- [ ] Missing auth checks on endpoints
- [ ] Unsafe deserialization
```

（第二个 skill 因 `disable-model-invocation: true`，不会出现在 `<available_skills>` 中，只能通过 `/skill:security-review` 显式调用。）
