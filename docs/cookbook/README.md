# Pandaria 生态 Cookbook

> 面向 AI Agent 基础设施开发者、架构师和运维工程师的完整指南。  
> 覆盖 Pandaria 核心运行时及其卫星项目（Emerald、Pawbun、Tavern、Constell）的架构原理、集成方式和运维实践。

---

## 导航

| 章节 | 内容 | 适合 |
|------|------|------|
| [第一章：Pandaria 核心架构](./01-architecture.md) | 多租户模型、Session 隔离、模块边界、依赖方向 | 想理解「Pandaria 是什么」的人 |
| [第二章：Agent Loop 与 Tool Use](./02-agent-loop.md) | LLM 原生 tool calling 协议、并行工具执行、Compaction、Prompt 构建 | 想理解 agent 内部运行机制的人 |
| [第三章：Hook 系统](./03-hooks.md) | 直接函数调用模型、阻断型/链式/观测型 hook、内置策略 | 想写自定义策略或排查 hook 行为的人 |
| [第四章：生态项目概览](./04-ecosystem.md) | Emerald（记忆）、Pawbun（工具）、Tavern（编排）、Constell（可观测性） | 想理解生态全景的人 |
| [第五章：集成指南](./05-integration.md) | 各项目的接入方式、HTTP adapter 实现、Session 生命周期管理 | 想把项目接起来的人 |
| [第六章：部署与运维](./06-deployment.md) | Docker Compose 全栈部署、健康检查、日志、监控、版本兼容性 | 想把生态跑在生产环境的人 |

---

## 快速开始

**5 分钟理解 Pandaria 生态**：

```
用户请求 (REST / SSE)
      │
      ▼
┌─────────────────────────────────────────────────┐
│                  Pandaria                        │
│  ┌──────────┐  ┌──────────┐  ┌───────────────┐  │
│  │Agent Loop│  │  Hook    │  │ MemoryStore   │  │
│  │ (turn)   │  │ System   │  │ (trait)       │──┼──► Emerald
│  └────┬─────┘  └──────────┘  └───────────────┘  │
│       │         ▲ 在线 guard 策略                 │
│       ▼         │ (在线 guard:                 │
│  ┌──────────┐  │  注入检测/内容审查/参数校验)    │
│  │  Tool    │  │                                │
│  │ Executor │  │  ┌───────────────┐             │
│  └────┬─────┘  │  │ HeirloomTool  │──┼──► Heirloom
│       │        │  │ (Phase 1+)    │  │
│       │        │  └───────────────┘  │
└───────┼──────────────────────────────────────────┘
        │
   ┌────┴────┐
   │ Pawbun  │  ← 工具注册、MCP 客户端、文件处理
   └─────────┘

┌──────────┐     ┌──────────┐
│  Tavern  │────►│ Pandaria │  ← 多 Agent 编排
└──────────┘     └─────┬────┘
                       │
                  ┌────┴────┐
                  │Tokencamp│  ← LLM 网关（替代 ai-provider）
                  └─────────┘

        ┌──────────┐
        │ Constell │  ← 可观测 + 评测 + Guard 离线检测
        └──────────┘

┌──────────┐
│ Aspectus │  ← 统一身份、认证、多租户（所有项目依赖）
└──────────┘
```

**先读哪个？**
- 新接触 → 第一章（架构）
- 要写代码 → 第二、三章（Agent Loop + Hook）
- 要接生态 → 第四、五章（生态项目 + 集成）
- 要部署 → 第六章

---

## 参考文档

- [Pandaria AGENTS.md](../../AGENTS.md) — 核心设计决策（ADR）和模块边界
- [生态概览](../ecosystem.md) — 卫星项目关系与全景图
- [生态集成 Spec](../specs/2026-05-28-ecosystem-integration-deepening.md) — 集成深化技术规格
- [Emerald MemoryStore Spec](../specs/2026-05-27-pandaria-emerald-memorystore.md) — Emerald HTTP adapter 接口契约
- [Runtime Openness Spec](../specs/2026-05-20-runtime-openness.md) — 工具即服务、Session 状态机、事件投递
