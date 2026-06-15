# tavern-flow-macros

`tavern-comp` 的过程宏 crate。提供方法级事件驱动编排的 DSL 宏。

## 提供的宏

### `#[start]` / `#[listen]` / `#[router]`

标注 impl 块中的方法，自动生成工作流步骤定义和 `FlowStepExecutor` 实现。

```rust
use tavern_flow_macros::FlowAgent;

#[derive(FlowAgent)]
#[flow(id = "content_writer", name = "内容撰稿人")]
struct ContentWriter;

#[tavern_flow_macros::async_trait]
impl ContentWriter {
    #[start]
    async fn draft(&self, topic: String) -> String {
        format!("关于 {} 的初稿...", topic)
    }

    #[listen("review_complete")]
    async fn revise(&self, draft: String, feedback: String) -> String {
        format!("修订后的版本: {}", draft)
    }
}
```

宏展开后生成 `tavern_comp::Workflow` + `tavern_comp::FlowStepExecutor` 胶水代码，实现方法级事件驱动的编排语义：
- `#[start]` — 工作流入口
- `#[listen("event_name")]` — 监听特定事件后执行
- `#[router("expression")]` — 条件路由

## 依赖

- `syn` — 语法解析
- `quote` — 代码生成

## 使用

`tavern-comp` 自动依赖此 crate，无需手动添加：

```toml
[dependencies]
tavern-comp = { path = "../tavern-comp" }
```
