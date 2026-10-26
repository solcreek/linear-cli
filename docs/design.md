# linearctl — command surface (as built, 2026-10-26)

Go + cobra, hand-written GraphQL over `net/http` against
`https://api.linear.app/graphql`. Every document is in `queries.go`; the tests
validate each against the vendored `graphql/schema.graphql` (`make schema`
refreshes it). No `@linear/sdk`, no code generation.
The CLI is the contract: `linear-tui` (OpenTUI) spawns `linearctl … --json` and
never holds the API key.

Naming follows our other tools (`crispctl`, `discordctl`). State moves with
`LINEARCTL_HOME`.

## Global flags

| flag | meaning |
|---|---|
| `-h`, `--help` | help on stdout, exit 0, before anything else |
| `--version` | top level only |
| `--json` | one JSON document on stdout |
| `--timeout N` | seconds per network request (default 30) |

## Exit codes

| code | name | meaning |
|---|---|---|
| 0 | ok | done, including "found nothing" |
| 1 | failed | network error, Linear error, not found (message names what) |
| 2 | usage | bad invocation; mutating command without exactly one of `--dry-run`/`--yes` |
| 3 | partial | results plus `problems` (e.g. some ids in a batch not found) |
| 4 | busy | the config or credentials lock is held; nothing changed |
| 5 | auth | no API key, or Linear rejected it. Fix: `linearctl auth login --yes` |
| 6 | rate-limited | Linear's request or complexity budget is spent; JSON has `retryAt` (RFC 3339) |

## Inputs every command treats the same way

- `ISSUE`: `ENG-123`, a UUID, or a `https://linear.app/<ws>/issue/ENG-123/...`
  URL. Anything else, or control characters, `?`, `#`, `%`: usage (2).
- `--team KEY`: team key (`ENG`); falls back to `LINEARCTL_TEAM`, then
  `default-team` in config.
- Names (`--state "In Progress"`, `--label bug`, `--assignee me|EMAIL`) go to
  Linear as names: `issue list` filters by name, and `issue create`/`update`
  resolve every name to an id in the same single request that checks the
  issue. No local cache is needed. Ambiguous name: failed (1), with a count.
- Long text (`--description`, `--body`) also comes as `--description-file PATH`
  or `-` for stdin.

## Commands

`M` = mutates (requires exactly one of `--dry-run` / `--yes`, never prompts).

### auth

    linearctl auth login  --yes|--dry-run   M   API key on stdin; checks it with `viewer`, stores it
    linearctl auth status                       viewer, organization, key source, rate-limit headroom
    linearctl auth logout --yes|--dry-run   M   removes the stored key; rerun → changed:false

Key precedence: `LINEAR_API_KEY` env > stored key, so Linux agent hosts need no
store at all. `auth login` stores in the macOS Keychain (service `linearctl`);
when `LINEARCTL_HOME` is set, or no keychain exists, in
`$LINEARCTL_HOME/config/credentials` / `$XDG_CONFIG_HOME/linearctl/credentials`
(0600). `auth status` never prints the key, only
`"source": "env"|"keychain"|"file"`.

Example: `pbpaste | linearctl auth login --yes`

### issue

    linearctl issue list   [--team KEY] [--assignee me|EMAIL|none] [--state NAME]...
                           [--state-type backlog|unstarted|started|completed|canceled]...
                           [--label NAME]... [--project NAME] [--cycle current|next|N]
                           [--query TEXT] [--updated-since DATE]
                           [--sort updated|created|priority] [--limit N] [--cursor C]
                           [--fields LIST]
    linearctl issue show   ISSUE [--comments[=N]] [--signed-urls SECONDS] [--fields LIST]   (alias: view)
    linearctl issue create --team KEY --title TEXT [--description TEXT|--description-file PATH|-]
                           [--assignee …] [--state NAME] [--priority 0-4] [--label NAME]...
                           [--project NAME] [--parent ISSUE] --id UUID
                           [--json-input PATH|-]                                         M
    linearctl issue update ISSUE [same fields as create, each optional]
                           [--add-label NAME]... [--remove-label NAME]...
                           [--json-input PATH|-]                                         M
    linearctl issue archive ISSUE                                                        M

- `issue list` default `--limit 25`; JSON `{items, total: null, shown, truncated,
  nextCursor}`. `total` is null because Linear connections have no count.
  `--query` uses `searchIssues`.
- `issue list` items (default fields): `id, identifier, title, state{name,type},
  assignee{name,email}, priority, labels[], project{name}, updatedAt, url`.
  `--fields identifier,title,state` trims them (A6).
- `issue show` adds `description`, `comments[]` (default 20), `attachments[]`,
  `parent`, `children[]`, `relations[]`. `--signed-urls 300` sends
  `public-file-urls-expire-in: 300`, so image URLs in the JSON open without a token.
- `issue create` is rerun-safe by construction: `--yes` requires `--id UUID`
  (usage, 2, without it). `--dry-run` without `--id` generates one and prints it
  in `id`, so the agent's flow is dry-run → copy `id` → `--yes --id`. If an issue
  with that id already exists, the result is that issue with `changed: false`.
  The same rule applies to `comment create`.
- `issue update`: fetches first and sends only fields that differ; nothing differs
  → `changed: false`, no request.
- `--json-input` takes Linear's `IssueCreateInput` / `IssueUpdateInput` as is
  (A11); flags given alongside override its fields.
- `--dry-run` resolves every name to an id and prints the exact mutation input it
  would send.

### comment

    linearctl comment list   ISSUE [--limit N] [--cursor C]
    linearctl comment create ISSUE (--body TEXT | --body-file PATH|-) --id UUID     M

### Lookups (what a picker or an agent needs to name things)

    linearctl team list    [--limit N]
    linearctl state list   [--team KEY]       # workflow states, ordered by position
    linearctl label list   [--team KEY] [--limit N]
    linearctl user list    [--active] [--limit N]
    linearctl project list [--team KEY] [--status TYPE]... [--limit N]
    linearctl cycle list   [--team KEY] [--limit N]

### Files

    linearctl file download URL [--output PATH]                                     M

Only `https://uploads.linear.app/…` URLs. Writes inside the current directory
(`inside()`, create-only-if-absent); default name from the URL's last segment.
An existing file with the same bytes → `changed: false`; different bytes → failed.

### Config

    linearctl config show
    linearctl config set KEY VALUE                                                  M

Keys: `default-team`. File: `$XDG_CONFIG_HOME/linearctl/config.json`,
replaced under `withLock`.

### Raw API (A11)

    linearctl api query  [--json-input PATH|-] [--signed-urls SECONDS]

Input is `{"query": "...", "variables": {...}}`. It refuses a document that
contains a `mutation` or `subscription` operation (usage, 2). Output is Linear's
`data` and `errors`, plus `complexity` from `X-Complexity`.

No raw mutations in v1: a raw mutation cannot be dry-run (nothing resolves or
previews it) or report `changed: false`, so writes go through the typed commands,
which do both. `api mutate` is added when a real write has no typed command.

### describe

    linearctl describe        # every command, flag, exit code as JSON

## Out of scope for v1

OAuth (needs a callback flow; API key covers agents), webhooks and live updates
(a later `watch --json` NDJSON stream for the TUI), multiple workspaces (a later
`--workspace NAME` selecting a stored key), attachments upload, documents,
initiatives.

## Fields that carry other people's text (agent context)

`title`, `description`, `comments[].body`, `labels[].name`, `project.name`,
`user.name`, `attachments[].title`, and everything under `api query` output.
