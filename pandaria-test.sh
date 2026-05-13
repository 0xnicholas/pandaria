#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# 优先级：release pandaria > debug pandaria > release pandaria-tui > debug pandaria-tui > cargo build
if [[ -f "target/release/pandaria" ]]; then
    exec "target/release/pandaria" "$@"
elif [[ -f "target/debug/pandaria" ]]; then
    exec "target/debug/pandaria" "$@"
elif [[ -f "target/release/pandaria-tui" ]]; then
    exec "target/release/pandaria-tui" "$@"
elif [[ -f "target/debug/pandaria-tui" ]]; then
    exec "target/debug/pandaria-tui" "$@"
else
    echo "Building pandaria (debug)..."
    cargo run --bin pandaria -- "$@"
fi
