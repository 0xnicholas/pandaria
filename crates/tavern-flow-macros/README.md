# tavern-flow-macros

`tavern-comp` 的过程宏 crate。提供方法级事件驱动编排的 DSL 宏，用于以 Rust 代码方式声明 Agent Team 的协作流程。

## 提供的宏

### `#[start]` / `#[listen]` / `#[router]`

标注 impl 块中的方法，自动生成工作流步骤定义和 `FlowStepExecutor` 实现。

```rust
use tavern_comp::{Flow, FlowError, flow_impl};

#[derive(Flow)]
struct ContentTeam {
    topic: String,
}

#[flow_impl(crate = "tavern_comp")]
impl ContentTeam {
    #[start]
    async fn research(&mut self,
    ) -> Result<String, FlowError> {
        Ok(format!("关于 {} 的研究结果...", self.topic))
    }

    #[listen("research")]
    async fn write(&mut self,
        notes: String,
    ) -> Result<String, FlowError> {
        Ok(format!("基于 {} 撰写文章", notes))
    }

    #[listen("write")]
    async fn edit(&mut self,
        draft: String,
    ) -> Result<String, FlowError> {
        Ok(format!("编辑后的版本: {}", draft))
    }
}
```

宏展开后生成 `tavern_comp::Workflow` + `tavern_comp::FlowStepExecutor` 胶水代码，实现方法级事件驱动的编排语义：

- `#[start]` — Agent Team 入口，对应一个 role 的初始 mission
- `#[listen("event_name")]` — 监听特定 mission 完成后执行
- `#[router("expression")]` — 条件路由，输出 label 触发下游分支

## 注意

当前宏生成的是基于 `Workflow` / `FlowStepExecutor` 的旧接口。下一阶段将迁移到 `Team` / `Squad` / `AgentExecutor` 新抽象，同时保持相似的声明式体验。

## 依赖

- `syn` — 语法解析
- `quote` — 代码生成

## 使用

`tavern-comp` 自动依赖此 crate，无需手动添加：

```toml
[dependencies]
tavern-comp = { path = "../tavern-comp" }
```
