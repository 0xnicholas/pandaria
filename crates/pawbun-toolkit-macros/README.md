# pawbun-toolkit-macros

`pawbun-toolkit` 的过程宏 crate。

## 提供的宏

### `#[pawbun_tool]`

为 impl 块标注的 struct 自动生成 `Tool` trait 实现。

```rust
use pawbun_toolkit::Tool;
use pawbun_toolkit_macros::pawbun_tool;

#[pawbun_tool(
    name = "hello_world",
    description = "返回问候语"
)]
impl HelloTool {
    pub fn run(&self, name: String) -> String {
        format!("Hello, {}!", name)
    }
}
```

宏自动从方法签名提取参数名称、类型和描述，生成 `Tool` trait 所需的 `name()`、`description()`、`parameters()` 和 `execute()` 方法。

## 依赖

- `syn` — 语法解析
- `quote` — 代码生成

## 使用

在 `pawbun-toolkit` 中启用 `macros` feature 即可自动引入：

```toml
[dependencies]
pawbun-toolkit = { path = "../pawbun-toolkit", features = ["macros"] }
```
