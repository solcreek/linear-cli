#!/usr/bin/env bash
# Measure one linearctl binary without the network: start time, end to end
# against bench/fake, peak RSS, size. Results go to bench/results/LABEL/.
#
#   bench/run.sh ./linearctl go-0.2.0
#   bench/compare.py bench/results/go-0.2.0 bench/results/rust-0.3.0
set -euo pipefail

bin=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
label=$2
root=$(cd "$(dirname "$0")/.." && pwd)
out=$root/bench/results/$label
runs=${RUNS:-300}
port=${PORT:-8787}
rm -rf "$out" && mkdir -p "$out"
work=$(mktemp -d)
trap 'kill $fake 2>/dev/null; wait $fake 2>/dev/null || true; rm -rf "$work"' EXIT

go build -o "$work/fake" "$root/bench/fake"
"$work/fake" -addr "127.0.0.1:$port" 2>/dev/null &
fake=$!
until curl -s -o /dev/null "http://127.0.0.1:$port/graphql"; do sleep 0.05; done

export LINEARCTL_HOME=$work/home LINEARCTL_API_URL=http://127.0.0.1:$port/graphql
export LINEAR_API_KEY=lin_api_bench000000000000000
unset LINEARCTL_DEMO LINEARCTL_META LINEARCTL_TEAM
mkdir -p "$work/home"

# 100 requests for one serve process.
for _ in $(seq 100); do echo '{"id":1,"argv":["issue","list","--limit","100","--json"]}'; done >"$work/serve.jsonl"

# name|arguments
scenarios=(
  "version|--version"
  "help|--help"
  "issue-list-help|issue list --help"
  "describe|describe --json"
  "issue-list|issue list --limit 100 --json"
  "issue-show|issue show ENG-1 --comments=100 --json"
  "demo-issue-list|issue list --demo --limit 100 --json"
  "serve-100|serve --stdio"
)

{
  echo "label: $label"
  echo "date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "host: $(hostname -s), $(uname -sm), $(sysctl -n machdep.cpu.brand_string 2>/dev/null || uname -p)"
  echo "load: $(uptime | sed 's/.*load averages*: //')"
  echo "binary: $bin"
  echo "version: $("$bin" --version)"
  echo "git: $(git -C "$root" rev-parse --short HEAD)"
  echo "size_bytes: $(wc -c <"$bin" | tr -d ' ')"
  echo "runs: $runs"
} >"$out/env.txt"

for s in "${scenarios[@]}"; do
  name=${s%%|*}
  args=${s#*|}
  input=()
  [ "$name" = serve-100 ] && input=(--input "$work/serve.jsonl")
  # A command that fails would be measuring its error path: refuse it.
  if [ "$name" = serve-100 ]; then "$bin" $args <"$work/serve.jsonl" >/dev/null; else "$bin" $args >/dev/null 2>&1; fi
  hyperfine -N --warmup 20 --runs "$runs" ${input[@]+"${input[@]}"} --style none \
    --export-json "$out/$name.json" --command-name "$name" "$bin $args" >/dev/null
  # Peak RSS: the largest of 10 runs, in bytes.
  peak=0
  for _ in $(seq 10); do
    # exec: time reports the binary itself, not the shell around it.
    run=(sh -c 'exec "$0" "$@" >/dev/null 2>&1' "$bin" $args)
    if [ "$(uname)" = Darwin ]; then
      rss=$(/usr/bin/time -l "${run[@]}" <"${input[1]:-/dev/null}" 2>&1 | awk '/maximum resident/ {print $1}')
    else
      rss=$(/usr/bin/time -v "${run[@]}" <"${input[1]:-/dev/null}" 2>&1 | awk '/Maximum resident/ {print $6 * 1024}')
    fi
    [ "${rss:-0}" -gt "$peak" ] && peak=$rss
  done
  echo "$name $peak" >>"$out/rss.txt"
  echo "measured $name"
done
python3 "$root/bench/compare.py" "$out" | tee "$out/summary.md"
