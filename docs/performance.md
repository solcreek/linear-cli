# Performance

## Current state (verified 2026-10-26)

Measured on a MacBook Air M3 (load average ~4), Go 1.27.1, linearctl at
`430093e`. A command's time is almost all network: one request to
api.linear.app costs ~150 ms, and everything linearctl does around it costs
~5 ms.

| what | cost | notes |
|---|---|---|
| process start (`--version`, `--help`) | 4.2 ms mean | `/usr/bin/true` is 0.9 ms; package init is 0.46 ms of it |
| `describe` | 4.9 ms | |
| `issue list --limit 100 --json`, local fake API | 6.4 ms end to end | 55 KB answer |
| `issue show --comments 100 --json`, local fake API | 5.7 ms | 53 KB answer |
| decode a 100-issue page | 0.50 ms | 95 MB/s, 8,461 allocs |
| flatten + list shape | 0.07 ms | |
| pretty-print the JSON | 0.38 ms | |
| reading the key from the Keychain | **~10 ms** | spawns `/usr/bin/security`; skipped when `LINEAR_API_KEY` is set |
| one request to api.linear.app | **~150 ms** (min), tail to 1 s | DNS 2 ms, TCP 15–20 ms, TLS +25 ms, server ~100–140 ms |
| linearctl vs curl, same request | 154.6 ms vs 146.0 ms (min) | |
| peak RSS | 12.7–15.6 MB | |
| binary | 12.9 MB (8.8 MB with `-s -w`) | stripping does not change start time |

Requests per command: reads 1; `issue create`/`update`/`archive` and
`comment create` 2 (resolve, then mutate) on one connection; `--limit 0`
one per page of 100.

## How to measure

    go test -run '^$' -bench . -benchmem .          # decode, shape, encode
    hyperfine -N --warmup 20 './linearctl --version'

End to end without the network: point `LINEARCTL_API_URL` at a local fake
that returns canned pages (the 2026-10-26 run used a 100-issue page and an
issue with 100 comments).

Linear's edge for these runs was Cloudflare `DFW`, origin behind Google
(`via: 1.1 google`); HTTP/2, and `alt-svc` offers HTTP/3.

Not yet measured, needs a real key: per-query complexity (`X-Complexity`),
whether answers are gzip-compressed, and server time by query (search is
rate-limited to 30/min and may be slower).
