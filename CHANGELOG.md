# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Until 1.0.0, a minor version may change the command surface; `--json` field
names are only ever added, never renamed or removed.

## [Unreleased]

## [0.9.0] - 2026-10-26

### Added

- `serve --stdio`: a request's optional `"stdin"` is its command's standard
  input, so `--json-input=-`, `--body-file=-` and the like work as on a command
  line (`""` is an empty one; without it, `-` is refused as before).
  `--concurrency N` (default 1, at most 32) runs up to N requests at once, sharing
  one key, connection pool and rate budget; answers come as each finishes.

### Fixed

- `auth login` inside `serve` no longer reads serve's own stdin, the request
  stream, for the key: it reads the request's `"stdin"`, or is refused.

## [0.8.0] - 2026-10-26

### Added

- `project show` carries `health`, `createdAt`, `updatedAt`, the teams' `name`,
  and `documents` (`id`, `title`, `url`), read in full 250 at a time.
- `project list` rows carry `slugId`, `startDate` and `progress`.

## [0.7.0] - 2026-10-26

### Added

- `issue list --blocked` and `--blocking` (Linear's hasBlockedByRelations and
  hasBlockingRelations; Linear ignores completed and canceled blockers), and
  `--completed-since WHEN`. `watch` takes them too.
- A `NOT_FOUND` or `AMBIGUOUS` failure from resolving a name says what was looked
  for, as `kind` and `name`. An unknown state or label adds `available`, the
  names the team can use (workspace labels and its own, no groups); a name that
  fits several things adds `candidates` with each one's `id`, `name` and
  `slugId`, `email`, `number`, `team` or `project` where it has one.
- `relation delete ISSUE OTHER`, without TYPE, deletes every relation between the
  two issues in either direction and lists each in `relations`. One that fails
  to delete is a partial result (exit 3) with `problems`.
- `cycle show --all-issues` returns every issue in the cycle, however many,
  250 a request; each issue row carries its state type, assignee and delegate.
  `--issues 0` still means none, as on `project show`.
- Demo mode has saved views: the seed's two, so `view list` and
  `issue list --view` work there. A view's filter in the seed is issue list
  flags, applied as issue list reads them.

### Changed

- `issue show` reads its labels, children, relations, inverse relations and
  attachments in full, 250 at a time; it stopped at 50 (labels at 20). Each
  relation names both issues with their titles. It adds `creator.id`,
  `cycle.name`, `cycle.startsAt` and `cycle.endsAt`, and `project.status`
  (`name`, `type`).
- Issue rows carry the team's `name` beside its `id` and `key`, and `issue show`'s
  relations carry each issue's `id`.

### Fixed

- `--label`, `--add-label` and `--remove-label` never resolve to a group label,
  which Linear cannot put on an issue: a label of the same name wins, and a name
  only a group carries is `NOT_FOUND`, its `available` leaving groups out.

## [0.6.0] - 2026-10-26

### Added

- Linear's saved custom views: `view list`, and `issue list --view VIEW` (a name
  or id) for an issue view's issues, Linear applying the view's own filter with
  any other flags on top. A project view is refused as such.
- `issue list --sort due-date|title|state` and `--reverse` (undated issues last
  either way); `--creator WHO`; `--due-before` and `--due-after DATE`. Issue rows
  carry `dueDate` and `estimate`. The default order sends the same request as
  before.
- `issue create` and `issue update` take `--estimate POINTS`, `--due-date DATE`
  and `--cycle CYCLE` (current, next, previous, a number or a UUID, within the
  issue's team), each with `none` to clear. Before, only `--json-input`
  reached them, with raw ids.

### Changed

- `issue create` takes the team from Linear's own `teamId` in `--json-input`
  when `--team` is not given (`--team` still wins). It used to demand `--team`.

### Fixed

- `watch` keeps working when Linear allows only one subscription per account
  (it has since 2026-10-26, closing every further one with 4003). Watch then
  subscribes to issue updates alone and catches creates and archives up every
  60 s, or `--reconcile` if shorter, instead of reconnecting every second and
  spending most of the hourly request budget on catch-ups.
- `watch` backs off on a connection that drops at once (1, 2, 4 … 30 s), not
  only on one that fails to open; the backoff starts over only after a
  connection stayed up a minute.
- `--project none`: on `issue create` and `issue update` it clears the project
  (undo can revert setting one); on `issue list` it matches issues with no
  project. `issue list` returned nothing for it, and update refused it.

## [0.5.0] - 2026-10-26

### Added

- A failure under `--json` carries `code`, a stable name for its kind:
  `USAGE`, `NOT_FOUND`, `AMBIGUOUS`, `LINEAR_ERROR`, `NETWORK`, `FAILED`,
  `BUSY`, `AUTH`, `RATE_LIMITED`, `CONFLICT`. Exit 1 covers five of them, which
  differed only in prose. `describe` lists them under `errorCodes` with their
  exit codes. `error` and `usage` are unchanged.
- A failure under `--json` carries `hint` when there is a next step: the
  command that lists what was not found (an issue, comment, project or cycle), the
  read to make after an `--if-updated-at` conflict, and, after a timeout, a lost
  connection or a 5xx from Linear, that rerunning the command is safe. Without
  `--json` it is a `hint: …` line on stderr. `error` and the exit codes are
  unchanged.
- An agent skill, `skills/linearctl/`, packaged as a Claude Code plugin
  (`.claude-plugin/plugin.json`). `make skill` generates it from AGENTS.md and
  the commands' `describe` and `--help`; `make test` fails when it is stale, or
  when AGENTS.md's list of mutating commands differs from `describe`.
- `issue create` and `issue update --state-type TYPE`: the team's first
  workflow state of that type, by position, as Linear's agent guide picks the
  `started` state when work begins. No workspace's state names needed.
- `--delegate me` on `issue create`, `issue update` and `issue list`: the
  token's own user, which must be an app (an agent); a person's key is refused
  with an explanation.

### Fixed

- `api query` prints Linear's answer when Linear answers with errors: `data`,
  `errors` as received (message, path, extensions such as `code` and
  `userPresentableMessage`) and `complexity`. Before, an answer without data
  was reduced to `{"error", "usage"}`; it now carries those too, and the exit
  codes are unchanged (1 without data, 3 with some).
- An unknown command exits 2 even with `--help` (`linearctl nope --help`,
  `linearctl issue nope -h`, `linearctl help nope`, and one argument holding
  a space, `"api query" --help`). It printed the parent's help and exited 0, so
  a mistyped command read as success.

## [0.4.0] - 2026-10-26

### Added

- `cycle show CYCLE` (current, next, previous, a number, or a UUID; `--team`
  picks among teams): Linear's own Cycle fields under Linear's names, among them
  `progress`, `currentProgress`, `progressHistory` and the day-by-day
  `scopeHistory`, `completedScopeHistory`, `inProgressScopeHistory`,
  `issueCountHistory` and `completedIssueCountHistory`, plus `issues` (paged
  with `--cursor`) and `uncompletedIssuesUponClose` as lists. Nothing is
  computed; `--fields` trims.
- `cycle list` rows carry the same fields as `cycle show`'s top level:
  `description`, `completedAt`, `isPrevious`, `isPast`, `isFuture`, `progress`,
  and `team.id`.
- `cycle create` (`--team`, `--starts-at`, `--ends-at`, `--name`,
  `--description`; `--yes` needs `--id`, so a rerun returns the existing cycle),
  `cycle update` (sends only what differs) and `cycle archive`. Linear's
  `cycleShiftAll` and `cycleStartUpcomingCycleToday` are left out: neither can
  be previewed, and running either twice moves cycles twice.
- `cycle list --active`, `--next`, `--previous`, `--past`, `--future` (Linear's
  `is*` states; several match any of them), `--starts-since` and
  `--ends-before`.

### Changed

- `issue list --cycle` also takes `previous` and a cycle's UUID, the words
  `cycle show` takes.

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

[Unreleased]: https://github.com/solcreek/linear-cli/compare/v0.9.0...HEAD
[0.9.0]: https://github.com/solcreek/linear-cli/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/solcreek/linear-cli/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/solcreek/linear-cli/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/solcreek/linear-cli/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/solcreek/linear-cli/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/solcreek/linear-cli/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/solcreek/linear-cli/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/solcreek/linear-cli/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/solcreek/linear-cli/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/solcreek/linear-cli/releases/tag/v0.1.0
