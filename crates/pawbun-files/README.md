# pawbun-files

多模态文件处理层。提供统一的、类型安全的文件加载与格式化抽象，支持 text/image/PDF/audio/video 多种媒体类型。

## 职责

- 统一文件表示（`File`、`MediaType`、`MediaContent`）
- 多源文件加载（Local / URL / Bytes）
- 文件校验与解析（`FileLoader`、`AsyncFileLoader`）
- LLM Provider 格式化（`ProviderFormat` — `OpenAiFormat`、`AnthropicFormat`、`GeminiFormat`、`AzureOpenAiFormat`）

## 架构分层

| 层 | 类型 | 职责 |
|---|---|---|
| Type | `File`, `MediaType`, `MediaContent` | 统一文件表示 |
| Source | `FileSource` | 抽象文件来源 |
| Loader | `FileLoader`, `AsyncFileLoader`, `DefaultFileLoader` | 读取、校验、解析 |
| Provider | `ProviderFormat`, `OpenAiFormat`, `AnthropicFormat`, `GeminiFormat`, `AzureOpenAiFormat` | LLM API 格式化 |

## 公开接口

| 模块 | 核心导出 |
|---|---|
| `file` | `File`、`FileKey` |
| `media` | `MediaType`、`MediaContent` |
| `provider` | `ProviderFormat` trait、`OpenAiFormat`、`AnthropicFormat`、`GeminiFormat`、`AzureOpenAiFormat` |
| `loader` | `FileLoader` trait、`DefaultFileLoader`、`AsyncFileLoader`（feature-gated） |
| `content` | `ContentBlock`、格式化工具 |
| `constraints` | 文件大小/类型约束 |

## 快速使用

```rust
use pawbun_files::{File, DefaultFileLoader, FileLoader, OpenAiFormat, ProviderFormat};

let loader = DefaultFileLoader::new();
let file = File::from_path("./chart.png").with_key("sales_chart");

let loaded = loader.load(&file).expect("load file");
let block = OpenAiFormat.format_content(&loaded.content).expect("format");
```

## 特性开关

| Feature | 说明 | 额外依赖 |
|---|---|---|
| `url-source` | HTTP 下载（`FileSource::Url`） | `reqwest` |
| `image-meta` | 图片尺寸提取 | `image` |
| `tracing` | tracing span 注入 | `tracing` |
| `tokio` | 异步文件加载 | `tokio`、`futures` |
| `full` | 启用所有特性 | — |

## 依赖

- `serde` / `serde_json` — 序列化
- `thiserror` — 错误类型
- `base64` — 内联 base64 编码
- `bytes` — 字节缓冲
- `reqwest`（可选）— URL 下载
- `image`（可选）— 图片元数据
- `tracing`（可选）— 可观测性
