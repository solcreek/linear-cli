.PHONY: build release test check cover live schema

build:
	cargo build

release:
	cargo build --release

# The unit tests, then the acceptance suite and the golden cases against the
# debug build: the behavior linearctl promises, checked from the outside.
test: build
	cargo test --quiet
	LINEARCTL_BIN=target/debug/linearctl go test -count=1 ./acceptance
	python3 tests/golden/run.py check target/debug/linearctl

# What CI runs: run it before every commit.
check:
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings
	@test -z "$$(gofmt -l acceptance bench)" || { gofmt -l acceptance bench; echo "gofmt: run gofmt -w on these"; exit 1; }
	go vet ./...
	$(MAKE) test

# Coverage of every test above, merged, on one instrumented binary.
cover:
	scripts/cover.sh

# Against the real API, changing a real team: LINEARCTL_LIVE_TEAM names a
# sandbox team, LINEAR_API_KEY a key for it. Never part of test or check.
live: build
	LINEARCTL_BIN=target/debug/linearctl go test -count=1 -v -run Live ./acceptance

# Refresh Linear's schema; the tests then check every query against it.
schema:
	curl -sfL -o graphql/schema.graphql https://raw.githubusercontent.com/linear/linear/master/packages/sdk/src/schema.graphql
