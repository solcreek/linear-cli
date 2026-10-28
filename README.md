# linearctl

[![Release](https://img.shields.io/github/v/release/solcreek/linear-cli)](https://github.com/solcreek/linear-cli/releases/latest)
[![npm version](https://img.shields.io/npm/v/@solcreek/linear-cli)](https://www.npmjs.com/package/@solcreek/linear-cli)
[![CI](https://github.com/solcreek/linear-cli/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/solcreek/linear-cli/actions/workflows/ci.yml)
[![MIT license](https://img.shields.io/github/license/solcreek/linear-cli)](LICENSE)

An agent-first CLI for [Linear](https://linear.app): list, search, show, create and
update issues and comments from a shell or an AI agent, with `--json` on every
command, `--dry-run`/`--yes` on every change, and a `describe` command that prints
the whole surface as JSON.

`linearctl` speaks GraphQL to `https://api.linear.app/graphql` directly, without
[`@linear/sdk`](https://github.com/linear/linear/tree/master/packages/sdk). The
SDK loads relations lazily, one request each, and an API key gets 2,500 requests
an hour; a `linearctl` read sends one request per page of up to 250. The
queries are written by hand and checked against Linear's published schema by the
tests.

## Install

```sh
npm install -g @solcreek/linear-cli                         # macOS, Linux, Windows
cargo install --git https://github.com/solcreek/linear-cli  # from source
```

Or download a binary from [Releases](https://github.com/solcreek/linear-cli/releases/latest)
(`checksums.txt` alongside). The npm package installs the native binary for
your platform, and its install script puts that binary where `linearctl`
points, so no Node runs when you call it (~3 ms a run). Where install scripts
are skipped (`--ignore-scripts`, pnpm, and npm versions that ask for
`--allow-scripts`), a small Node shim runs the same binary instead (~34 ms).
To keep the fast path on npm 11.19 and later:

```sh
npm install -g @solcreek/linear-cli --allow-scripts=@solcreek/linear-cli
```

A program that depends on the npm package can run the native binary itself,
with no Node in between. Its path is stable, the one the package's own shim
uses:

```js
const exe = process.platform === "win32" ? "linearctl.exe" : "linearctl"
const bin = require.resolve(`@solcreek/linear-cli-${process.platform}-${process.arch}/bin/${exe}`)
```

## Try it without a key

```sh
linearctl --demo issue list
LINEARCTL_DEMO=1 linearctl issue show ENG-142
```

Demo mode answers from a fictional workspace built into the binary, Northwind,
with no API key and no network. Every command sends its usual GraphQL query and
gets the answer from that workspace, so the output has exactly the shapes of
real output. Writes are refused; `--dry-run` works.

## Authenticate

Create a personal API key at <https://linear.app/settings/account/security>, then
either export it:

```sh
export LINEAR_API_KEY=lin_api_…
```

or store it (macOS Keychain, or `credentials.json` with mode 0600 elsewhere). The
key is read from stdin, never from an argument:

```sh
pbpaste | linearctl auth login --yes
linearctl auth status
```

`LINEAR_API_KEY` wins over a stored key.

## Use

```sh
linearctl issue list --assignee me --state-type started
linearctl issue list --team ENG --label bug --updated-since 7d --json
linearctl issue list --query "login timeout" --limit 10
linearctl issue list --any-label bug --any-label regression --without-label wontfix
linearctl issue show ENG-123
linearctl issue show ENG-123 --signed-urls 300 --json   # image URLs that open without the key

linearctl issue create --team ENG --title "Fix login timeout" --label bug --dry-run --json
linearctl issue create --team ENG --title "Fix login timeout" --label bug --id <id from dry run> --yes
linearctl issue update ENG-123 --state "In Progress" --assignee me --yes
linearctl comment create ENG-123 --body-file notes.md --id <uuid> --yes

linearctl team list
linearctl state list --team ENG
linearctl label list --team ENG
echo '{"query":"query { viewer { name } }"}' | linearctl api query --json-input -
```

Every command has `--help` with examples. `linearctl describe` prints all commands,
flags and exit codes as JSON.

### Commands

| command | does | changes things |
|---|---|---|
| `auth login` / `status` / `logout` | store, check, remove the API key | login, logout |
| `issue list` | list and search issues; filter by team, assignee, state, label, project, cycle, date; `--fields` adds each row's `parent` and relations | |
| `issue show` | one issue with description, comments, sub-issues, relations, attachments | |
| `issue create` / `update` / `archive` | change issues | yes |
| `comment list` / `create` | read and post comments | create |
| `team` / `state` / `label` / `user` / `project` / `cycle list` | the names other commands take | |
| `project show` | one project with its overview, documents, milestones and first 50 issues; `--all-issues` for every one, `issuesTruncated` says when some were left out | |
| `file download` | save an image or attachment from `uploads.linear.app` | yes |
| `config show` / `set` | `default-team` | set |
| `api query` | send your own GraphQL query (read-only) | |
| `serve --stdio` | run many invocations from one process, as JSON lines (~80 ms less per call) | |
| `watch` | stream changes to the issues a filter matches, as NDJSON (Linear's subscriptions, else polling) | |

## For agents

[AGENTS.md](AGENTS.md) is the context to give an agent. In short:

- Always pass `--json`. Into a pipe it is compact, one document on stdout.
- Every command that changes something takes `--dry-run` or `--yes`, exactly one,
  and never prompts. A rerun that has nothing to do succeeds with
  `"changed": false`.
- `issue create` and `comment create` need `--id UUID` with `--yes`. `--dry-run`
  prints one; retrying with the same id returns the existing item instead of a
  duplicate.
- Lists are `{items, total, shown, truncated, nextCursor}`; continue with
  `--cursor`, trim with `--fields` and `--limit`.
- `issue list --blocked` is Linear's `hasBlockedByRelations`, and Linear
  decides what it matches: an open issue that an open issue blocks (open:
  triage, backlog, unstarted or started). A completed or canceled blocker
  does not count; `--blocking` is the same from the other end. To see each
  blocker and its state, add `--fields identifier,inverseRelations`.
- A failure is `{error, usage, code}`, plus `hint` when there is a next step:
  the command that finds what was not found, or that a rerun is safe. Branch on
  `code` (`NOT_FOUND`, `AMBIGUOUS`, `NETWORK`, …; `describe` lists them all),
  not on the words of `error`. A name that did not resolve adds `kind` and
  `name`, `available` (for a state or label) or `candidates` (for `AMBIGUOUS`).

| exit | meaning |
|---|---|
| 0 | ok, including "found nothing" |
| 1 | failed: a network error, a Linear error, not found |
| 2 | usage: fix the invocation, do not retry it as is |
| 3 | partial: `api query` data with `problems` |
| 4 | busy: a config lock is held; retry |
| 5 | auth: no API key, or Linear rejected it |
| 6 | rate-limited: retry after `retryAt` |
| 7 | conflict: `--if-updated-at` no longer holds; read it again |

## Calling linearctl from a program

- Pass `--json` and read one document from stdout (one line, in a pipe). Decide
  by the exit code, then by `code` in a failure (`describe` lists the codes).
- Give linearctl a stdin pipe only when you send input (`--json-input -`,
  `--body-file -`, `--description-file -`, `auth login`); otherwise give it none
  or `/dev/null`. linearctl can exit before reading stdin, after a usage error
  or for a command that reads none, and your write to the pipe then fails with
  EPIPE: ignore that error and use the exit code.
- Retry on `NETWORK`, and on `RATE_LIMITED` after `retryAt`. Changes are safe to
  rerun (`changed: false`); `issue create`, `comment create` and `cycle create`
  need `--id` for that.
- Making many calls: `serve --stdio` answers them from one process, about 80 ms
  less each. A request's `"stdin"` is its command's standard input (for
  `--json-input=-`), and `--concurrency N` runs up to N at once, answers matched
  by `id`.
- From the npm package, run the native binary directly (see Install) to skip
  Node.

## Speed

A command's time is almost all the request to Linear (~150 ms). Starting,
decoding and printing take ~4 ms: 2.7 ms to start, ~1.3 ms for a 100-issue page. Measurements
and what was tried: [docs/performance.md](docs/performance.md).

## Develop

linearctl is written in Rust; the acceptance tests and the benchmark's fake
Linear are Go.

```sh
make test     # unit tests, then acceptance/ and tests/golden against the binary
make check    # what CI runs: rustfmt, clippy, gofmt, go vet, then make test
make cover    # coverage of all of the above, merged
make live     # against the real API: LINEARCTL_LIVE_TEAM=SANDBOX LINEAR_API_KEY=…
make schema   # refresh graphql/schema.graphql from Linear
```

`acceptance/` runs the built binary against a fake Linear that checks every
request against Linear's schema, with `LINEARCTL_HOME` pointing at a temporary
directory, so the tests never touch the network, the Keychain or your config.
`tests/golden` holds what every command prints offline (help, errors, demo
mode), byte for byte. The design is in [docs/design.md](docs/design.md).

## License

[MIT](LICENSE)
