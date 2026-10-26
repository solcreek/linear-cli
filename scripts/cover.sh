#!/usr/bin/env bash
# Coverage of the Rust build by every test there is, merged: the unit tests,
# the black-box suite (go test with LINEARCTL_BIN) and the golden cases, all
# run against one instrumented binary.
#
#   scripts/cover.sh            # summary
#   scripts/cover.sh --html     # and target/llvm-cov/html/index.html
#
# Needs cargo-llvm-cov, and llvm-cov/llvm-profdata of rustc's LLVM version:
# rustup's llvm-tools-preview, or LLVM_COV and LLVM_PROFDATA set.
set -euo pipefail
cd "$(dirname "$0")/.."
suite=${SUITE:-./acceptance}
[ -d "$suite" ] || suite=./cmd/linearctl

if [ -z "${LLVM_COV:-}" ] && ! rustup component list --installed 2>/dev/null | grep -q llvm-tools; then
  llvm=$(rustc --version --verbose | awk '/LLVM version/ {split($3, v, "."); print v[1]}')
  for dir in "/opt/homebrew/opt/llvm@$llvm/bin" "/usr/lib/llvm-$llvm/bin"; do
    if [ -x "$dir/llvm-cov" ]; then
      export LLVM_COV=$dir/llvm-cov LLVM_PROFDATA=$dir/llvm-profdata
      break
    fi
  done
fi

eval "$(cargo llvm-cov show-env --sh 2>/dev/null)"
cargo llvm-cov clean --workspace
cargo build --quiet
cargo test --quiet 2>&1 | grep -E '^test result' || true
bin=$PWD/target/debug/linearctl
LINEARCTL_BIN=$bin go test -count=1 "$suite"
python3 tests/golden/run.py check "$bin"
cargo llvm-cov report --summary-only --ignore-filename-regex '/(\.cargo|rustc|rustlib)/' | tail -20
if [ "${1:-}" = --html ]; then
  cargo llvm-cov report --html --ignore-filename-regex '/(\.cargo|rustc|rustlib)/'
fi
