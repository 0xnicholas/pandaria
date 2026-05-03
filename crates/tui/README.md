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
| Ctrl+O | Toggle tool calls |
| Ctrl+T | Toggle thinking blocks |
| Ctrl+L | Select model |
| Ctrl+S | Session list |
| `/` | Open command palette |

## Commands

`/quit`, `/new`, `/switch`, `/list`, `/model`, `/clear`, `/connect`, `/auth`, `/tokens`, `/help`
