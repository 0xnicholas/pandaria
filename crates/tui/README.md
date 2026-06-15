# pandaria-tui

Terminal-based chat client for the pandaria agent runtime.

## Usage

```bash
cargo run -p tui -- --url http://localhost:8080 --token sk-xxxxx
```

Or set environment variables:

```bash
export PANDARIA_URL=http://localhost:8080
export PANDARIA_TOKEN=sk-xxxxx
cargo run -p tui
```

## Keybindings

| Key | Action |
|---|---|
| Enter | Submit message |
| Esc | Cancel / interrupt |
| Ctrl+C | Quit |
| Ctrl+X | Open external editor |
| Ctrl+O | Toggle tool calls |
| Ctrl+T | Toggle thinking blocks |
| Ctrl+P | Previous model |
| Ctrl+N | Next model |
| Ctrl+S | Session list |
| Ctrl+Shift+P | Command palette (any state) |
| Ctrl+Shift+- | Redo |
| Ctrl+] | Character jump |
| `!` | Bash mode prefix |
| `!!` | Bash mode (force) |
| `/` | Slash commands |

## Commands

`/quit`, `/new`, `/switch`, `/list`, `/model`, `/clear`, `/connect`, `/auth`, `/tokens`, `/help`

## Features

- **双队列输入系统**：steer 队列（高优先级注入）+ followUp 队列（agent 完成后继续对话）
- **Bash 模式**：`!command` 或 `!!command` 直接在 shell 执行命令并捕获输出
- **外部编辑器**：Ctrl+X 打开 `$EDITOR` 编写消息，保存后自动发送
- **命令面板解耦**：Ctrl+Shift+P 在任意 UI 状态下可用
- **模型循环切换**：Ctrl+P/N 在已配置模型中循环选择
- **Redo**：Ctrl+Shift+- 重做已撤销的操作
- **字符跳转**：Ctrl+] 快速跳转到指定字符位置
- **CompactionSummary**：渲染上下文压缩摘要的消息类型
