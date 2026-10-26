# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Until 1.0.0, a minor version may change the command surface; `--json` field
names are only ever added, never renamed or removed.

## [Unreleased]

## [0.3.0] - 2026-10-26

### Changed

- linearctl is now written in Rust. Commands, flags, `--help`, `describe`,
  `--json` output, error messages and exit codes are those of 0.2.1: 397
  outputs recorded from 0.2.1 (help, usage errors, config and credentials,
  every read and dry run in demo mode, `serve`, completion) match byte for
  byte (`tests/golden`), and the 0.2.1 tests that run the binary against a
  fake Linear pass unchanged (`acceptance/`). Against 0.2.0 as released (0.2.1
  changed only its version string), on
  one machine in one session: 1.6× faster to start (2.7 vs 4.3 ms), 1.7× for
  a 100-issue `issue list` (4.0 vs 6.7 ms), 2× for 100 requests through
  `serve --stdio` (133 vs 270 ms), half the memory, and a 2.5 MiB binary
  instead of 9.2 MiB ([docs/performance.md](docs/performance.md)).
- Release binaries are still one self-contained file per platform (Linux:
  static, musl); the archives, `checksums.txt` and the npm packages keep
  their names.
- `linearctl … | head` ends quietly, as before; an empty value among a
  repeated flag's (`--label x --label ""`) is refused, as before.
- From source: `cargo install --git https://github.com/solcreek/linear-cli`
  (`go install` no longer applies).
- The demo workspace moved from `cmd/linearctl/demo/workspace.json` to
  `demo/workspace.json`.

### Known differences

- After `cannot reach HOST:`, the reason is the operating system's or the
  HTTP client's wording, not Go's: `connection refused` rather than
  `dial tcp 127.0.0.1:9: connect: connection refused`.
- `watch` connects its WebSocket directly; `HTTPS_PROXY` applies to every
  other request, as before.
- An answer from Linear with a field of an unexpected type is read as far as
  it goes, where 0.2.0 failed with `unexpected answer from Linear`.

## [0.2.1] - 2026-10-26

### Fixed

- Releases publish to npm by themselves again (trusted publishing). 0.2.0's
  npm job failed on a path bug in the publish script, and 0.2.0 was published
  by hand.
- The main npm package is published only after the registry serves every
  platform package; with 0.2.0, installing in the first minute could leave out
  the binary.
- Prereleases go to npm under the `next` tag, never `latest`.

## [0.2.0] - 2026-10-26

### Added

- Demo mode: `--demo` or `LINEARCTL_DEMO=1` answers every read from a fictional
  workspace built into the binary (Northwind, 17 issues), with no API key and no
  network. Commands send their usual queries to an in-process resolver, so the
  output has the real shapes; `--yes` is refused and `--dry-run` works.
- `serve --stdio`: answers linearctl invocations as JSON lines, `{id, argv}` in
  and `{id, exit, result, stdout, stderr}` out, from one process with one
  connection pool and one key read: ~80 ms less per call than starting
  linearctl each time (measured live).
- `watch`: streams changes to the issues an `issue list` filter matches, one
  JSON line per event (`ready`, `upsert`, `remove` with a reason, `heartbeat`
  with the rate limit left, `gap` with a replay after a reconnect or a fall
  back to polling). It uses Linear's GraphQL subscriptions over WebSocket
  (undocumented, working with a personal API key) and checks each batch of
  changed issues against the filter with one request; `--since` replays first.
- `relation create` and `relation delete ISSUE TYPE OTHER` for blocks,
  blocked-by, duplicate, related and similar. Creating a relation that exists,
  or deleting one that does not, is `changed: false`.
- `issue show` lists incoming relations as `inverseRelations`; relations carry
  their id.
- Project milestones: `--milestone NAME|none` on `issue create` and
  `issue update`, `issue list --milestone`, `milestone list --project`, and
  `projectMilestone` in issue rows.
- Agent delegation: `--delegate NAME|EMAIL|none` on `issue create` and
  `issue update` (app users only), `issue list --delegate` (also `any`,
  `none`), `delegate` in issue rows, and `user list --app`.
- `project show PROJECT` (overview content, status, milestones, issues) and
  `project update` (name, description, content). PROJECT is a URL, slugId,
  UUID or exact name; a name two projects share is an error that lists both.
- `issue unarchive`.
- `notification list [--type TYPE]`: your inbox, with `readAt` (null when
  unread), the actor and the issue.
- `issue update --parent none` detaches an issue from its parent.
- `issue list --attachment-url URL`: the issues that link a URL, such as a pull
  request. A caller that must wait for Linear to link one polls this.
- `comment create --parent COMMENT` to reply in a thread; `comment update` and
  `comment delete` (a comment that is not there is `changed: false`,
  `found: false`).
- `issue list --priority`, repeatable.
- `issue update --if-updated-at TIME` changes the issue only if its `updatedAt`
  is still TIME, and otherwise exits with the new code 7 (conflict).
- `--meta` (or `LINEARCTL_META=1`) adds `meta` to any `--json` result: the
  requests the command sent, the rate limit left after them, and the last
  query's complexity. It costs no extra request.
- `signedUrlsExpireAt` in `issue show` and `api query` results made with
  `--signed-urls`.
- `issue list --id ISSUE`, repeatable, to read several issues in one request;
  ids that do not come back are listed in `problems` (exit 3).
- `issue list --query TEXT --include-comments` passes Linear's `includeComments`
  to the search.
- Nested objects in issue rows (`state`, `assignee`, `team`, `project`,
  `cycle`, `labels`) carry their `id`, and states and labels their `color`.
- "What belongs in linearctl" in docs/design.md, and a test that keeps
  workspace conventions out of the source.

### Changed

- `issue list --team` may be repeated, matching any of the teams.

### Fixed

- `issue create` and `issue update` found a label only when its case matched
  (`--label bug` missed "Bug"); they now ignore case, as `issue list` does.

## [0.1.0] - 2026-10-26

### Added

- First release. Issues: `issue list` (filters by team, assignee, state, state
  type, label, project, cycle and update time; `--query` search; `--sort`),
  `issue show` (also `issue view`), `issue create`, `issue update` and
  `issue archive`.
- Label filters: `--label` (all of), `--any-label` (any of) and
  `--without-label` (none of).
- `comment list` and `comment create`.
- Lookups: `team list`, `state list`, `label list`, `user list`,
  `project list`, `cycle list`.
- `file download` for images and attachments on uploads.linear.app, and
  `issue show --signed-urls` for URLs that open without the API key.
- `auth login`, `auth status`, `auth logout`; `LINEAR_API_KEY` wins over a key
  stored in the macOS Keychain or `credentials.json`.
- `config show` and `config set default-team`.
- `api query` for read-only GraphQL; mutations are refused.
- `describe`: every command, flag, alias and exit code as JSON.
- `--json` on every command, compact into a pipe; `--dry-run`/`--yes` on every
  change; `--id` makes `issue create` and `comment create` safe to retry; exit
  codes 5 (auth) and 6 (rate-limited, with `retryAt`).
- Releases for linux, darwin and windows, and the npm package
  `@solcreek/linear-cli` with one native binary per platform.

[Unreleased]: https://github.com/solcreek/linear-cli/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/solcreek/linear-cli/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/solcreek/linear-cli/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/solcreek/linear-cli/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/solcreek/linear-cli/releases/tag/v0.1.0
