#!/usr/bin/env bash
# Run Emerald integration tests — one-shot wrapper.
#
# Supports three modes:
#   1. Memory mode (default) — auto-start Emerald in-memory test server.
#      No Docker needed. Fastest. Data lost on exit.
#   2. Docker mode — start Emerald via docker-compose.emerald.yml.
#   3. External mode — connect to an already-running Emerald instance.
#
# Usage:
#   ./scripts/run-emerald-tests.sh [OPTIONS] [-- CARGO_ARGS]
#
# Options:
#   -h, --help         Show this help
#   --memory           Use in-memory Emerald server (default)
#   --docker           Use Docker Compose to start Emerald
#   --no-docker        Connect to an external Emerald (env vars must be set)
#   --keep             Keep server/container running after tests
#   --env-file PATH    Load env from file (default: .env.emerald)
#   --compose PATH     Docker Compose file (default: docker-compose.emerald.yml)
#
# Environment variables (loaded from --env-file or shell):
#   EMERALD_BASE_URL       Emerald URL (default: http://localhost:9999)
#   EMERALD_API_KEY        API key (default: em_test)
#   EMERALD_REPO           Path to Emerald repo (default: ~/Documents/build-whatever/Emerald)
#
# Examples:
#   # Default: auto-start Emerald in-memory server, test, stop
#   ./scripts/run-emerald-tests.sh
#
#   # Use Docker full stack
#   ./scripts/run-emerald-tests.sh --docker
#
#   # Connect to local Emerald already running on :9999
#   ./scripts/run-emerald-tests.sh --no-docker
#
#   # Keep server alive for debugging
#   ./scripts/run-emerald-tests.sh --keep
#
#   # Pass extra args to cargo test
#   ./scripts/run-emerald-tests.sh -- --nocapture test_e2e_remember

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# ---------------------------------------------------------------------------
# Defaults
# ---------------------------------------------------------------------------
MODE="memory"
COMPOSE_FILE="${PROJECT_ROOT}/docker-compose.emerald.yml"
ENV_FILE="${PROJECT_ROOT}/.env.emerald"
KEEP=false
HEALTH_PATH="/v1/health"
MAX_WAIT=60
PID_FILE=""
TEARDOWN_NEEDED=false

# ---------------------------------------------------------------------------
# Parse args
# ---------------------------------------------------------------------------
CARGO_ARGS=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        -h|--help)
            cat <<'EOF'
Run Emerald integration tests — one-shot wrapper.

Supports three modes:
  1. Memory mode (default) — auto-start Emerald in-memory test server.
     No Docker needed. Fastest. Data lost on exit.
  2. Docker mode — start Emerald via docker-compose.emerald.yml.
  3. External mode — connect to an already-running Emerald instance.

Usage:
  ./scripts/run-emerald-tests.sh [OPTIONS] [-- CARGO_ARGS]

Options:
  -h, --help         Show this help
  --memory           Use in-memory Emerald server (default)
  --docker           Use Docker Compose to start Emerald
  --no-docker        Connect to an external Emerald (env vars must be set)
  --keep             Keep server/container running after tests
  --env-file PATH    Load env from file (default: .env.emerald)
  --compose PATH     Docker Compose file (default: docker-compose.emerald.yml)

Environment variables (loaded from --env-file or shell):
  EMERALD_BASE_URL       Emerald URL (default: http://localhost:9999)
  EMERALD_API_KEY        API key (default: em_test)
  EMERALD_REPO           Path to Emerald repo

Examples:
  # Default: auto-start Emerald in-memory server, test, stop
  ./scripts/run-emerald-tests.sh

  # Use Docker full stack
  ./scripts/run-emerald-tests.sh --docker

  # Connect to local Emerald already running on :9999
  ./scripts/run-emerald-tests.sh --no-docker

  # Keep server alive for debugging
  ./scripts/run-emerald-tests.sh --keep

  # Pass extra args to cargo test
  ./scripts/run-emerald-tests.sh -- --nocapture test_e2e_remember
EOF
            exit 0
            ;;
        --memory)
            MODE="memory"
            shift
            ;;
        --docker)
            MODE="docker"
            shift
            ;;
        --no-docker)
            MODE="external"
            shift
            ;;
        --keep)
            KEEP=true
            shift
            ;;
        --env-file)
            ENV_FILE="$2"
            shift 2
            ;;
        --compose)
            COMPOSE_FILE="$2"
            shift 2
            ;;
        --)
            shift
            CARGO_ARGS+=("$@")
            break
            ;;
        -*)
            echo "Unknown option: $1" >&2
            echo "Run with --help for usage." >&2
            exit 1
            ;;
        *)
            CARGO_ARGS+=("$1")
            shift
            ;;
    esac
done

# ---------------------------------------------------------------------------
# Load env file if present
# ---------------------------------------------------------------------------
if [[ -f "$ENV_FILE" ]]; then
    set -a
    # shellcheck source=/dev/null
    source "$ENV_FILE"
    set +a
fi

EMERALD_BASE_URL="${EMERALD_BASE_URL:-http://localhost:9999}"
EMERALD_API_KEY="${EMERALD_API_KEY:-em_test}"
EMERALD_REPO="${EMERALD_REPO:-${HOME}/Documents/build-whatever/Emerald}"

# ---------------------------------------------------------------------------
# Helper: wait for health endpoint
# ---------------------------------------------------------------------------
wait_for_health() {
    local url="$1"
    echo "==> Waiting for Emerald at ${url}${HEALTH_PATH} ..."
    for ((i = 1; i <= MAX_WAIT; i++)); do
        if curl -sf "${url}${HEALTH_PATH}" >/dev/null 2>&1; then
            echo "==> Emerald is ready (${i}s)"
            return 0
        fi
        sleep 1
    done
    echo "==> ERROR: Emerald failed to become healthy in ${MAX_WAIT}s" >&2
    return 1
}

# ---------------------------------------------------------------------------
# Helper: teardown on exit
# ---------------------------------------------------------------------------
cleanup() {
    local exit_code=$?
    if [[ "$KEEP" == true ]]; then
        echo "==> Kept server running. Tear down manually."
        if [[ -n "${PID_FILE:-}" && -f "$PID_FILE" ]]; then
            echo "    kill \$(cat $PID_FILE)"
        fi
        if [[ "$MODE" == "docker" ]]; then
            echo "    docker compose -f '$COMPOSE_FILE' down"
        fi
        exit $exit_code
    fi

    if [[ "$TEARDOWN_NEEDED" == true && "$MODE" == "memory" ]]; then
        if [[ -n "${PID_FILE:-}" && -f "$PID_FILE" ]]; then
            local pid
            pid="$(cat "$PID_FILE")"
            echo "==> Stopping in-memory Emerald (PID $pid)..."
            kill "$pid" 2>/dev/null || true
            rm -f "$PID_FILE"
        fi
    fi

    if [[ "$TEARDOWN_NEEDED" == true && "$MODE" == "docker" ]]; then
        echo "==> Stopping Docker Compose Emerald..."
        docker compose -f "$COMPOSE_FILE" down
    fi

    exit $exit_code
}
trap cleanup EXIT INT TERM

# ---------------------------------------------------------------------------
# MODE: memory — auto-start in-memory test server
# ---------------------------------------------------------------------------
if [[ "$MODE" == "memory" ]]; then
    if ! command -v python3 >/dev/null 2>&1; then
        echo "ERROR: python3 not found. Install Python 3.12+ or use --docker/--no-docker." >&2
        exit 1
    fi

    # Find or clone Emerald repo
    if [[ ! -d "$EMERALD_REPO" ]]; then
        echo "==> Emerald repo not found at $EMERALD_REPO"
        read -r -p "Clone from https://github.com/earendil-works/emerald.git? [Y/n] " ans
        if [[ "${ans:-Y}" =~ ^[Nn]$ ]]; then
            echo "Set EMERALD_REPO to an existing clone, or use --docker/--no-docker." >&2
            exit 1
        fi
        mkdir -p "$(dirname "$EMERALD_REPO")"
        git clone https://github.com/earendil-works/emerald.git "$EMERALD_REPO"
        (cd "$EMERALD_REPO" && git checkout v0.2.0)
    fi

    # Ensure dependencies are installed
    if ! python3 -c "import emerald" 2>/dev/null; then
        echo "==> Installing Emerald dependencies..."
        (cd "$EMERALD_REPO" && pip install -e ".")
    fi

    echo "==> Starting Emerald in-memory server..."
    PID_FILE="$(mktemp)"
    (
        cd "$EMERALD_REPO"
        python3 scripts/test_server.py >/dev/null 2>&1 &
        echo $! > "$PID_FILE"
    )

    TEARDOWN_NEEDED=true
    EMERALD_BASE_URL="http://localhost:9999"
    EMERALD_API_KEY="em_test"

    if ! wait_for_health "$EMERALD_BASE_URL"; then
        echo "==> Memory mode failed to start. Check $EMERALD_REPO/scripts/test_server.py" >&2
        exit 1
    fi

# ---------------------------------------------------------------------------
# MODE: docker — Docker Compose full stack
# ---------------------------------------------------------------------------
elif [[ "$MODE" == "docker" ]]; then
    if ! command -v docker >/dev/null 2>&1; then
        echo "ERROR: docker not found. Install Docker or use --memory/--no-docker." >&2
        exit 1
    fi

    echo "==> Starting Emerald via Docker Compose..."
    docker compose -f "$COMPOSE_FILE" up -d
    TEARDOWN_NEEDED=true

    # Resolve actual mapped port
    local_port=$(docker compose -f "$COMPOSE_FILE" port emerald 8000 2>/dev/null | cut -d: -f2 || echo "")
    if [[ -n "$local_port" && "$local_port" != "8000" ]]; then
        EMERALD_BASE_URL="http://localhost:${local_port}"
    else
        EMERALD_BASE_URL="${EMERALD_BASE_URL:-http://localhost:8000}"
    fi

    if ! wait_for_health "$EMERALD_BASE_URL"; then
        docker compose -f "$COMPOSE_FILE" logs --tail=50
        exit 1
    fi

# ---------------------------------------------------------------------------
# MODE: external — already running
# ---------------------------------------------------------------------------
elif [[ "$MODE" == "external" ]]; then
    if [[ -z "${PANDARIA_TEST_EMERALD_URL:-}" && "$EMERALD_BASE_URL" == "http://localhost:9999" ]]; then
        # If user explicitly passed --no-docker but didn't set env, try to probe localhost
        echo "==> --no-docker: checking for Emerald at ${EMERALD_BASE_URL}..."
        if ! curl -sf "${EMERALD_BASE_URL}${HEALTH_PATH}" >/dev/null 2>&1; then
            EMERALD_BASE_URL="http://localhost:8000"
            echo "==> Trying ${EMERALD_BASE_URL}..."
            if ! curl -sf "${EMERALD_BASE_URL}${HEALTH_PATH}" >/dev/null 2>&1; then
                echo "ERROR: --no-docker requires a running Emerald server." >&2
                echo "Set PANDARIA_TEST_EMERALD_URL or EMERALD_BASE_URL." >&2
                exit 1
            fi
        fi
    else
        EMERALD_BASE_URL="${PANDARIA_TEST_EMERALD_URL:-$EMERALD_BASE_URL}"
    fi
    echo "==> Using external Emerald at $EMERALD_BASE_URL"
fi

# ---------------------------------------------------------------------------
# Run tests
# ---------------------------------------------------------------------------
echo "==> Running integration tests..."
echo "    URL:  $EMERALD_BASE_URL"
echo "    Args: ${CARGO_ARGS[*]:-(default)}"

export PANDARIA_TEST_EMERALD_URL="$EMERALD_BASE_URL"
export PANDARIA_TEST_EMERALD_API_KEY="$EMERALD_API_KEY"

cd "$PROJECT_ROOT"
cargo test -p agent-core --test integration_emerald -- --test-threads=1 "${CARGO_ARGS[@]+${CARGO_ARGS[@]}}"
