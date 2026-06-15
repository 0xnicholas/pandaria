# tavern-core

Tavern 工作流引擎的核心类型定义。提供 Agent 配置、工作计划、工具注册等共享类型的抽象边界。

## 职责

- 定义 `AgentConfig` — Agent 的完整 YAML 配置模型（model、instructions、skills、constraints、memory）
- 定义 `Plan` / `PlanStep` — 工作计划与步骤结构
- 定义 `ToolRegistry` / `ToolHandler` trait — 工具注册与调用的抽象边界
- 提供配置序列化/反序列化（YAML）

## 公开接口

| 类型 | 说明 |
|---|---|
| `AgentConfig` | Agent 配置（id, name, description, model, instructions, skills, constraints, memory） |
| `AgentSummary` | Agent 摘要信息 |
| `ManagerConfig` | 管理器配置 |
| `ModelConfig` | LLM 模型配置（provider, name, temperature 等） |
| `Plan` / `PlanStep` | 工作计划定义 |
| `PlanningConfig` | 计划生成配置 |
| `Process` | 处理配置 |
| `SkillConfig` | 技能配置 |
| `ToolRunner` | 工具运行器配置 |
| `MemoryConfig` | 记忆系统配置 |
| `ToolRegistry` trait | 工具发现抽象 |
| `ToolHandler` trait | 工具调用抽象 |
| `ToolResult` | 工具执行结果 |
| `ToolError` | 工具错误类型 |
| `ContentPart` | 内容片段类型 |

## 配置示例

```yaml
id: researcher
name: 研究员
description: 擅长信息检索

model:
  provider: openai
  name: gpt-4o
  temperature: 0.3

instructions: |
  你是一个研究助理。

skills:
  - id: web_search
    config:
      max_results: 5

constraints:
  - 回答必须使用中文

memory:
  enabled: true
  max_context_turns: 10
```

## 依赖

- `serde` / `serde_json` — 序列化
- `thiserror` — 错误类型
- `async-trait` — async trait 支持

## 边界

- **不实现**具体的工具执行逻辑 — 由 `pawbun-toolkit` 或调用方提供
- **不实现**工作流引擎 — 由 `tavern-comp` 实现
- **纯类型层** — 无 IO、无持久化、无运行时
