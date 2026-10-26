# Performance

## Current state (verified 2026-10-26)

linearctl is the Rust build after 0.2.0. Measured on a MacBook Air M3 (load
average ~4–5), Rust 1.98.1, against 0.2.0 built as it was released (Go
1.27.1, `-trimpath -s -w`), back to back in one session; 300 runs a scenario,
medians. Raw results: `bench/results/go-0.2.0-release` and `bench/results/rust`.
A command's time is still almost all network: one request to api.linear.app
costs ~150 ms; everything linearctl does around it now costs ~4 ms.

| what | Go 0.2.0 | Rust | |
|---|---:|---:|---:|
| process start (`--version`) | 4.25 ms | 2.69 ms | 1.6× |
| `--help`, `issue list --help` | 4.3–4.4 ms | 2.8 ms | 1.6× |
| `describe --json` | 5.34 ms | 3.49 ms | 1.5× |
| `issue list --limit 100 --json`, `bench/fake` | 6.69 ms | 3.96 ms | 1.7× |
| `issue show --comments=100 --json`, `bench/fake` | 5.52 ms | 3.47 ms | 1.6× |
| `issue list --demo --limit 100 --json` | 5.31 ms | 3.41 ms | 1.6× |
| 100 × `issue list --limit 100` through one `serve --stdio` | 270 ms | 133 ms | 2.0× |
| peak RSS | 13.7–21.4 MiB | 6.6–9.8 MiB | ~0.5× |
| binary | 9.15 MiB | 2.47 MiB | 3.7× smaller |
| clean release build | 5.8 s | 35.8 s (fat LTO) | |
| rebuild after touching `issue.rs` (debug) | 0.3 s | 0.6 s | |
| tests, from a clean tree | 6.8 s (`go test`) | 19.9 s (`make test`, compiling the tests included) | |
| coverage, all tests merged | 90.8% of statements | 94.4% of lines | |

Of the Rust build's ~1.3 ms for a 100-issue page past start-up (61 KB):
~0.4 ms for the local request, ~0.4 ms to parse it (with the Go-compatible
syntax check, 0.04 ms), ~0.3 ms to shape and write it. Both builds link
Security.framework for the system's certificates; the proof of concept,
with bundled roots instead, started in 2.5 ms: not worth losing the
Keychain's trust settings (a company's root certificate, for one).

| what | cost | notes |
|---|---|---|
| reading the key from the Keychain | **~10 ms** | spawns `/usr/bin/security`; skipped when `LINEAR_API_KEY` is set |
| one request to api.linear.app | **~150 ms** (min), tail to 1 s | DNS 2 ms, TCP 15–20 ms, TLS +25 ms, server ~100–140 ms |

`--json` into a pipe is compact (since `e44bd08`): 35% fewer bytes for a
100-issue page (84,080 → 54,655), 30% with `--fields`, 17% for an issue with
100 comments. At a terminal it stays indented.

Installed from npm (0.1.0, the Go build, measured 2026-10-26): 4.0 ms a run
when the install script replaced the shim with the binary, 34.3 ms through
the Node shim when install scripts were skipped (the Node start dominates).

Many calls from one program (measured 2026-10-26, live API, 15 sequential
`issue list --limit 1`): through one `serve --stdio` process, median 141 ms a
call (the first, which opens the connection, 484 ms); one process per call,
median 220 ms. serve saves ~80 ms a call: start-up, TCP and TLS.

Requests per command: reads 1; `issue create`/`update`/`archive` and
`comment create` 2 (resolve, then mutate) on one connection; `--limit 0`
one per page of 100.

## How to measure

One binary, every scenario (start, help, describe, list and show against
`bench/fake`, demo, 100 requests through `serve --stdio`), with peak RSS and
size; then build and test times and coverage:

    bench/run.sh target/release/linearctl LABEL      # → bench/results/LABEL/
    bench/build.sh rust LABEL
    bench/compare.py bench/results/A bench/results/B # before/after table

`bench/fake` answers IssueList with a 100-issue page and IssueShow with an
issue with 100 comments, so a run measures linearctl, not the network. For a
Go build: `git archive v0.2.0 | tar -x -C DIR`, build it there, and
`bench/build.sh go LABEL DIR`. `bench/results/go-0.2.0` is the first
baseline, an unstripped development build at `532295a` under heavier load
(the start-up figures are a millisecond higher; stripping does not change
them).

Linear's edge for these runs was Cloudflare `DFW`, origin behind Google
(`via: 1.1 google`); HTTP/2, and `alt-svc` offers HTTP/3.

Not yet measured, needs a real key: per-query complexity (`X-Complexity`),
whether answers are gzip-compressed, and server time by query (search is
rate-limited to 30/min and may be slower).

## Before Rust: the Go build (0.2.0 and earlier)

Measured 2026-10-26 at `430093e`, load average ~4: process start 4.2 ms
(package init 0.46 ms of it), `describe` 4.9 ms; against a local fake,
`issue list --limit 100 --json` 6.4 ms and `issue show --comments 100` 5.7 ms;
a 100-issue page decoded in 0.50 ms (95 MB/s, 8,461 allocations), was shaped
in 0.07 ms and pretty-printed in 0.38 ms; peak RSS 12.7–15.6 MB; the binary
12.9 MB (8.8 MB with `-s -w`). linearctl vs curl, same live request: 154.6 vs
146.0 ms (min).

## Tried and rejected

- **Persisting TLS session tickets across runs** (2026-10-26): no gain.
  Cold vs resumed against api.linear.app, 40 runs each, twice: min 151.4 /
  150.3 ms cold, 152.7 / 153.4 ms resumed. TLS 1.3 resumption takes the same
  one round trip as a full handshake; only 0-RTT would save it, and Go's
  crypto/tls did not send early data (rustls can, but Linear would have to
  accept it, and a replayed mutation is not safe).
- **Reading the Keychain through Security.framework** instead of spawning
  `security` (would save ~10 ms): the item's ACL trusts the tool that made
  it, so linearctl reading it directly would raise a macOS permission dialog,
  again after every upgrade of an unsigned binary. Agents set
  `LINEAR_API_KEY` and skip the Keychain entirely.

## Open, needs a real key

- Page size above 100 for `--limit 0` (fewer round trips). Linear documents
  no maximum `first`, only 10,000 complexity points per query; by its formula
  an `issueRow` is ~30 points (20 of them `labels(first: 20)`), so 250 per
  page is ~7,400. Linear checks auth before arguments, so this cannot be
  probed without a key.
