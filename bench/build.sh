#!/usr/bin/env bash
# Build and test times, and coverage, for the Rust tree, or for a Go tree
# (0.2.0 and before: git archive v0.2.0 | tar -x -C DIR). Writes
# bench/results/LABEL/build.txt.
#
#   bench/build.sh rust rust
#   bench/build.sh go go-0.2.0-release DIR
set -euo pipefail
lang=$1
label=$2
root=$(cd "$(dirname "$0")/.." && pwd)
src=$(cd "${3:-$root}" && pwd)
out=$root/bench/results/$label
mkdir -p "$out"
cd "$src"

secs() { # secs CMD...: wall seconds of one run, output discarded
  local s e
  s=$(python3 -c 'import time; print(time.time())')
  "$@" >/dev/null 2>&1
  e=$(python3 -c 'import time; print(time.time())')
  python3 -c "print(f'{$e - $s:.1f}')"
}

{
  echo "date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "load: $(uptime | sed 's/.*load averages*: //')"
  case $lang in
  go)
    echo "toolchain: $(go version)"
    cache=$(mktemp -d)
    echo "clean build: $(GOCACHE=$cache secs go build -o /dev/null ./cmd/linearctl) s"
    touch cmd/linearctl/issue.go
    echo "incremental build (touch issue.go): $(GOCACHE=$cache secs go build -o /dev/null ./cmd/linearctl) s"
    rm -rf "$cache"
    cache=$(mktemp -d)
    echo "clean release build (-trimpath -s -w): $(GOCACHE=$cache secs go build -trimpath -ldflags '-s -w' -o /dev/null ./cmd/linearctl) s"
    rm -rf "$cache"
    echo "test suite (go test -count=1): $(secs go test -count=1 ./...) s"
    echo "coverage (statements): $(make -s cover 2>/dev/null | tail -1 | awk '{print $NF}')"
    ;;
  rust)
    echo "toolchain: $(rustc --version)"
    cargo clean -q
    echo "clean release build: $(secs cargo build --release --locked) s"
    touch src/commands/issue.rs
    echo "incremental release build (touch issue.rs): $(secs cargo build --release --locked) s"
    echo "clean debug build: $(cargo clean -q; secs cargo build --locked) s"
    touch src/commands/issue.rs
    echo "incremental debug build (touch issue.rs): $(secs cargo build --locked) s"
    echo "test suite (make test): $(secs make test) s"
    echo "coverage (lines): $(scripts/cover.sh 2>/dev/null | tail -1 | awk '{print $(NF-3)}')"
    ;;
  esac
} | tee "$out/build.txt"
