.PHONY: build test check cover schema

build:
	go build -o linearctl ./cmd/linearctl

test:
	go test ./...

# What CI runs: run it before every commit.
check:
	@test -z "$$(gofmt -l .)" || { gofmt -l .; echo "gofmt: run gofmt -w on these"; exit 1; }
	go vet ./...
	go test -count=1 ./...

# Coverage of the in-process tests and of every binary run, merged.
cover:
	rm -rf .cover && mkdir -p .cover/bin .cover/unit
	LINEARCTL_COVER_DIR=$(CURDIR)/.cover/bin go test -count=1 -cover ./cmd/linearctl -args -test.gocoverdir=$(CURDIR)/.cover/unit
	go tool covdata textfmt -i=$$(ls -d .cover/bin/run* .cover/unit | paste -sd, -) -o .cover/profile.txt
	go tool cover -func=.cover/profile.txt | tail -1

# Refresh Linear's schema; the tests then check every query against it.
schema:
	curl -sfL -o graphql/schema.graphql https://raw.githubusercontent.com/linear/linear/master/packages/sdk/src/schema.graphql
