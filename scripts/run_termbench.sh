#!/usr/bin/env bash
# Run pterminal benchmarks and optionally TermMark inside pterminal.
# Usage: ./scripts/run_termbench.sh [--termbench]
#
# Without arguments: runs pterminal's built-in bench only.
# With --termbench:   also runs TermMark inside the pterminal GUI.
#                     Automatically downloads and builds termbench if needed.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
CLI="$PROJECT_DIR/target/release/pterminal-cli"
RUN_TERMBENCH=false

# parse args
while [[ $# -gt 0 ]]; do
  case "$1" in
    --termbench) RUN_TERMBENCH=true; shift ;;
    *) echo "Unknown option: $1"; exit 1 ;;
  esac
done

# build cli if needed
if [[ ! -x "$CLI" ]]; then
  echo "Building pterminal-cli (release)..."
  cargo build --release -p pterminal-cli --manifest-path "$PROJECT_DIR/Cargo.toml"
fi

# ============================================================
# 1. pterminal built-in bench (no GUI needed)
# ============================================================
echo "=== pterminal Built-in Bench ==="
BENCH_JSON=$("$CLI" bench --cols 120 --rows 40 --iterations 200 2>/dev/null)

# parse and print summary using python (available on macOS)
python3 -c "
import json, sys
data = json.loads(sys.stdin.read())
params = data['params']
print(f'  Grid: {params[\"cols\"]}x{params[\"rows\"]}  Iterations: {params[\"iterations\"]}')
print()
for b in data.get('benchmarks', []):
    name = b['name']
    avg = b['avg_ms']
    total = b.get('total_ms', avg * b.get('iterations', params['iterations']))
    tp = b.get('throughput_mib_s')
    tp_str = f'  ({tp:.1f} MiB/s)' if tp else ''
    print(f'  {name:30s} {avg:8.3f} ms/iter  total {total:8.1f} ms{tp_str}')
    # show render pipeline breakdown
    if 'stages_avg_ms' in b:
        print()
        for stage, ms in sorted(b['stages_avg_ms'].items(), key=lambda x: -x[1]):
            print(f'    {stage:28s} {ms:.3f} ms/iter')
" <<< "$BENCH_JSON"
echo "================================="

# ============================================================
# 2. TermMark (optional, requires GUI)
# ============================================================
if ! $RUN_TERMBENCH; then
  exit 0
fi

# --- ensure termbench binary exists, download & build if not ---
TERMBENCH_ROOT="$PROJECT_DIR/.tmp_bench"
TERMBENCH_DIR="$TERMBENCH_ROOT/termbench"
TERMBENCH_BIN="$TERMBENCH_DIR/termbench_release_clang"

if [[ -x "$TERMBENCH_BIN" ]]; then
  echo "Using cached termbench: $TERMBENCH_BIN"
else
  echo "Downloading and building termbench..."
  mkdir -p "$TERMBENCH_ROOT"
  if [[ ! -d "$TERMBENCH_DIR/.git" ]]; then
    rm -rf "$TERMBENCH_DIR"
    git clone --depth 1 https://github.com/cmuratori/termbench.git "$TERMBENCH_DIR" 2>&1 | tail -1
  fi
  (cd "$TERMBENCH_DIR" && bash build.sh)
  if [[ ! -x "$TERMBENCH_BIN" ]]; then
    echo "ERROR: failed to build termbench (clang++ required)" >&2
    exit 1
  fi
  echo "termbench built: $TERMBENCH_BIN"
fi

echo ""
echo "=== TermMark Benchmark ==="

# ensure pterminal GUI is running
LAUNCHED_HERE=false
if ! "$CLI" ping >/dev/null 2>&1; then
  echo "Starting pterminal..."
  nohup cargo run --release --manifest-path "$PROJECT_DIR/Cargo.toml" >/dev/null 2>&1 &
  LAUNCHED_HERE=true

  for i in $(seq 1 30); do
    if "$CLI" ping >/dev/null 2>&1; then break; fi
    sleep 0.5
  done

  if ! "$CLI" ping >/dev/null 2>&1; then
    echo "ERROR: pterminal did not start within 15 seconds" >&2
    exit 1
  fi
  echo "pterminal is ready."
else
  echo "pterminal is already running."
fi

MARKER_PREFIX="__TERMBENCH_DONE_"
MARKER_SUFFIX="$$"
MARKER="${MARKER_PREFIX}${MARKER_SUFFIX}"

echo "Running termbench in pterminal..."
"$CLI" send "$TERMBENCH_BIN; printf '%s%s\n' '$MARKER_PREFIX' '$MARKER_SUFFIX'
" >/dev/null

# poll screen until marker appears (timeout 300s)
TIMEOUT=300
ELAPSED=0
INTERVAL=5
PING_FAILS=0
MAX_PING_FAILS=5

while (( ELAPSED < TIMEOUT )); do
  SCREEN=$("$CLI" read-screen 2>/dev/null || true)
  if printf '%s' "$SCREEN" | grep -q "\\\\n${MARKER}\\\\n"; then
    printf "\r\033[K"
    break
  fi
  if "$CLI" ping >/dev/null 2>&1; then
    PING_FAILS=0
  else
    PING_FAILS=$(( PING_FAILS + 1 ))
    if (( PING_FAILS >= MAX_PING_FAILS )); then
      printf "\n"
      echo "ERROR: IPC unreachable during benchmark (${PING_FAILS} consecutive ping failures)" >&2
      echo "pterminal may still be running, but IPC is not responding." >&2
      exit 1
    fi
    printf "\r\033[KIPC ping failed (%d/%d), retrying..." "$PING_FAILS" "$MAX_PING_FAILS"
    sleep "$INTERVAL"
    ELAPSED=$(( ELAPSED + INTERVAL ))
    continue
  fi
  printf "\r\033[KWaiting for termbench... %ds elapsed (timeout %ds)" "$ELAPSED" "$TIMEOUT"
  sleep "$INTERVAL"
  ELAPSED=$(( ELAPSED + INTERVAL ))
done

if (( ELAPSED >= TIMEOUT )); then
  printf "\n"
  echo "ERROR: termbench did not finish within ${TIMEOUT}s" >&2
  exit 1
fi

# extract and print results
SCREEN=$("$CLI" read-screen 2>/dev/null | sed 's/\\n/\n/g')

echo ""
echo "=== TermMark Results (pterminal) ==="
echo "$SCREEN" | grep -E '(CPU:|VT support:)' || true
echo ""
# parse each test line: "TestName: Xs (Ygb/s)"
echo "$SCREEN" | grep -E '(ManyLine:|LongLine:|FGPerChar:|FGBGPerChar:|TermMarkV2)' | while IFS= read -r line; do
  # extract name, time, throughput
  name=$(echo "$line" | sed -E 's/^[[:space:]]*([^:]+):.*/\1/')
  time=$(echo "$line" | sed -E 's/.*:[[:space:]]*([0-9.]+)s.*/\1/')
  tp=$(echo "$line" | sed -E 's/.*\(([0-9.]+)gb\/s\).*/\1/')
  printf "  %-20s %10ss  (%s gb/s)\n" "$name" "$time" "$tp"
done
echo "====================================="

# close pterminal if we started it
if $LAUNCHED_HERE; then
  "$CLI" send "exit
" >/dev/null 2>&1 || true
  sleep 1
  if "$CLI" ping >/dev/null 2>&1; then
    PTERM_PID=$(pgrep -f "target/release/pterminal$" | head -1)
    if [[ -n "$PTERM_PID" ]]; then
      kill "$PTERM_PID" 2>/dev/null || true
    fi
  fi
fi
