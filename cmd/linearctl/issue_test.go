package main

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"slices"
	"strings"
	"testing"
	"time"
)

// jsonOf is v as compact JSON, for comparing variables.
func jsonOf(v any) string {
	b, _ := json.Marshal(v)
	return string(b)
}

func listFake(t *testing.T, pages ...map[string]any) *fake {
	n := 0
	answer := func(vars map[string]any) (int, any, map[string]string) {
		p := pages[min(n, len(pages)-1)]
		n++
		return 200, map[string]any{"data": map[string]any{"issues": p, "searchIssues": p}}, nil
	}
	return newFake(t).onFunc("IssueList", answer).onFunc("IssueSearch", answer)
}

func TestIssueListDefaults(t *testing.T) {
	t.Parallel()
	api := listFake(t, conn("", issueNode("ENG-1", "One"), issueNode("ENG-2", "Two")))
	r := runWith(t, opts{api: api, key: testKey}, "issue", "list", "--json")
	d := decode(t, r.out)
	if r.code != 0 || d["shown"] != 2.0 || d["truncated"] != false || d["total"] != nil || d["nextCursor"] != nil {
		t.Fatalf("exit %d, %q %q", r.code, r.out, r.err)
	}
	item := d["items"].([]any)[0].(map[string]any)
	if !reflect.DeepEqual(item["labels"], []any{map[string]any{"name": "bug"}}) {
		t.Errorf("labels not flattened: %v", item["labels"])
	}
	v := api.last("IssueList").Vars
	if v["first"] != 25.0 || v["orderBy"] != "updatedAt" || v["filter"] != nil || v["after"] != nil {
		t.Errorf("vars %v", v)
	}
}

func TestIssueListFilters(t *testing.T) {
	t.Parallel()
	cases := []struct {
		args []string
		want string // the filter, as JSON
	}{
		{[]string{"--team", "eng"}, `{"team":{"key":{"eq":"ENG"}}}`},
		{[]string{"--team", "eng", "--team", "Web"}, `{"team":{"key":{"in":["ENG","WEB"]}}}`},
		{[]string{"--priority", "urgent", "--priority", "2"}, `{"priority":{"in":[1,2]}}`},
		{[]string{"--assignee", "me"}, `{"assignee":{"isMe":{"eq":true}}}`},
		{[]string{"--assignee", "none"}, `{"assignee":{"null":true}}`},
		{[]string{"--assignee", "ada@example.com"}, `{"assignee":{"email":{"eqIgnoreCase":"ada@example.com"}}}`},
		{[]string{"--state", "Todo", "--state", "In Progress"}, `{"state":{"name":{"in":["Todo","In Progress"]}}}`},
		{[]string{"--state-type", "started", "--state", "QA"}, `{"state":{"name":{"in":["QA"]},"type":{"in":["started"]}}}`},
		{[]string{"--state-type", "duplicate"}, `{"state":{"type":{"in":["duplicate"]}}}`},
		{[]string{"--label", "bug", "--label", "p1"}, `{"and":[{"labels":{"some":{"name":{"eqIgnoreCase":"bug"}}}},{"labels":{"some":{"name":{"eqIgnoreCase":"p1"}}}}]}`},
		{[]string{"--any-label", "bug", "--any-label", "p1"}, `{"and":[{"or":[{"labels":{"some":{"name":{"eqIgnoreCase":"bug"}}}},{"labels":{"some":{"name":{"eqIgnoreCase":"p1"}}}}]}]}`},
		{[]string{"--without-label", "wontfix"}, `{"and":[{"labels":{"every":{"name":{"neqIgnoreCase":"wontfix"}}}}]}`},
		{[]string{"--label", "bug", "--any-label", "p1", "--any-label", "p2", "--without-label", "dup"}, `{"and":[{"labels":{"some":{"name":{"eqIgnoreCase":"bug"}}}},{"or":[{"labels":{"some":{"name":{"eqIgnoreCase":"p1"}}}},{"labels":{"some":{"name":{"eqIgnoreCase":"p2"}}}}]},{"labels":{"every":{"name":{"neqIgnoreCase":"dup"}}}}]}`},
		{[]string{"--project", "Q4 launch"}, `{"project":{"name":{"eqIgnoreCase":"Q4 launch"}}}`},
		{[]string{"--attachment-url", "https://github.com/acme/app/pull/42"}, `{"attachments":{"some":{"url":{"eq":"https://github.com/acme/app/pull/42"}}}}`},
		{[]string{"--cycle", "current"}, `{"cycle":{"isActive":{"eq":true}}}`},
		{[]string{"--cycle", "next"}, `{"cycle":{"isNext":{"eq":true}}}`},
		{[]string{"--cycle", "12"}, `{"cycle":{"number":{"eq":12}}}`},
		{[]string{"--updated-since", "2026-10-01"}, `{"updatedAt":{"gte":"2026-10-01T00:00:00Z"}}`},
		{[]string{"--updated-since", "2026-10-01T08:00:00+08:00"}, `{"updatedAt":{"gte":"2026-10-01T00:00:00Z"}}`},
	}
	for _, c := range cases {
		api := listFake(t, conn(""))
		r := runWith(t, opts{api: api, key: testKey}, append([]string{"issue", "list", "--json"}, c.args...)...)
		if r.code != 0 {
			t.Errorf("%v: exit %d, %s", c.args, r.code, r.err)
			continue
		}
		if got := jsonOf(api.last("IssueList").Vars["filter"]); got != c.want {
			t.Errorf("%v: filter %s, want %s", c.args, got, c.want)
		}
	}
}

func TestIssueListRelativeSince(t *testing.T) {
	t.Parallel()
	api := listFake(t, conn(""))
	runWith(t, opts{api: api, key: testKey}, "issue", "list", "--updated-since", "7d")
	f := api.last("IssueList").Vars["filter"].(map[string]any)
	since, err := time.Parse(time.RFC3339, f["updatedAt"].(map[string]any)["gte"].(string))
	if err != nil || time.Since(since) < 7*24*time.Hour-time.Minute || time.Since(since) > 7*24*time.Hour+time.Minute {
		t.Errorf("7d became %v", f)
	}
}

func TestIssueListRefusesBadFilters(t *testing.T) {
	t.Parallel()
	cases := [][]string{
		{"--assignee", "ada"}, {"--state-type", "done"}, {"--cycle", "last"}, {"--updated-since", "yesterday"},
		{"--sort", "title"}, {"--query", "x", "--sort", "priority"}, {"--fields", "identifier,nope"},
		{"--team", "E N G"}, {"--priority", "soon"}, {"--priority", "5"}, {"--state", "a\x07"}, {"--limit", "-1"}, {"--any-label", ""}, {"--without-label", "a\x07"}, {"--attachment-url", "github.com/x"},
	}
	api := newFake(t)
	for _, args := range cases {
		r := runWith(t, opts{api: api, key: testKey}, append([]string{"issue", "list", "--json"}, args...)...)
		if r.code != exitUsage || decode(t, r.out)["usage"] != true {
			t.Errorf("%v: exit %d, %q", args, r.code, r.out)
		}
	}
	if len(api.calls()) != 0 {
		t.Errorf("refused invocations called Linear: %v", api.ops())
	}
}

func TestIssueListTeamFallsBackToEnvThenConfig(t *testing.T) {
	t.Parallel()
	dir := t.TempDir()
	must(t, os.MkdirAll(filepath.Join(dir, ".home", "config"), 0o700))
	must(t, os.WriteFile(filepath.Join(dir, ".home", "config", "config.json"), []byte(`{"default-team":"web"}`), 0o600))
	for _, c := range []struct {
		env  []string
		args []string
		want string
	}{
		{nil, nil, "WEB"},
		{[]string{"LINEARCTL_TEAM=ops"}, nil, "OPS"},
		{[]string{"LINEARCTL_TEAM=ops"}, []string{"--team", "eng"}, "ENG"},
	} {
		api := listFake(t, conn(""))
		r := runWith(t, opts{dir: dir, api: api, key: testKey, env: c.env}, append([]string{"issue", "list"}, c.args...)...)
		got := jsonOf(api.last("IssueList").Vars["filter"])
		if r.code != 0 || got != `{"team":{"key":{"eq":"`+c.want+`"}}}` {
			t.Errorf("%v %v: exit %d, filter %s", c.env, c.args, r.code, got)
		}
	}
}

func TestIssueListSearchAndSort(t *testing.T) {
	t.Parallel()
	api := listFake(t, conn(""))
	runWith(t, opts{api: api, key: testKey}, "issue", "list", "--query", "login timeout")
	v := api.last("IssueSearch").Vars
	if v["term"] != "login timeout" || v["orderBy"] != nil {
		t.Errorf("search ranks by relevance by default: %v", v)
	}
	runWith(t, opts{api: api, key: testKey}, "issue", "list", "--query", "x", "--include-comments")
	if v := api.last("IssueSearch").Vars; v["includeComments"] != true {
		t.Errorf("--include-comments: %v", v)
	}
	if r := runWith(t, opts{api: api, key: testKey}, "issue", "list", "--include-comments"); r.code != exitUsage {
		t.Errorf("--include-comments without --query: exit %d", r.code)
	}
	runWith(t, opts{api: api, key: testKey}, "issue", "list", "--query", "x", "--sort", "created")
	if v := api.last("IssueSearch").Vars; v["orderBy"] != "createdAt" {
		t.Errorf("search --sort created: %v", v)
	}
	runWith(t, opts{api: api, key: testKey}, "issue", "list", "--sort", "priority")
	if v := api.last("IssueList").Vars; v["orderBy"] != nil || jsonOf(v["sort"]) != `[{"priority":{"order":"Descending"}}]` {
		t.Errorf("--sort priority: %v", v)
	}
}

func TestIssueListPages(t *testing.T) {
	t.Parallel()
	// A capped list says so, and where to continue.
	api := listFake(t, conn("c1", issueNode("ENG-1", "One"), issueNode("ENG-2", "Two")))
	r := runWith(t, opts{api: api, key: testKey}, "issue", "list", "--limit", "2", "--json")
	d := decode(t, r.out)
	if r.code != 0 || d["truncated"] != true || d["nextCursor"] != "c1" || !strings.Contains(r.err, "--cursor c1") {
		t.Errorf("capped: exit %d, %q, stderr %q", r.code, r.out, r.err)
	}
	if api.last("IssueList").Vars["first"] != 2.0 {
		t.Errorf("asked for more than --limit: %v", api.last("IssueList").Vars)
	}

	// --limit 0 follows every page.
	api = listFake(t, conn("c1", issueNode("ENG-1", "One")), conn("", issueNode("ENG-2", "Two")))
	r = runWith(t, opts{api: api, key: testKey}, "issue", "list", "--limit", "0", "--cursor", "c0", "--json")
	calls := api.calls()
	if d := decode(t, r.out); r.code != 0 || d["shown"] != 2.0 || d["truncated"] != false || len(calls) != 2 {
		t.Fatalf("all: exit %d, %q, %d calls", r.code, r.out, len(calls))
	}
	if calls[0].Vars["after"] != "c0" || calls[1].Vars["after"] != "c1" || calls[0].Vars["first"] != float64(pageSize) {
		t.Errorf("cursors %v, %v", calls[0].Vars, calls[1].Vars)
	}

	// A limit above one page asks for the rest on the second.
	// (Linear caps a page at 250.)
	full := []any{}
	for i := range pageSize {
		full = append(full, issueNode(fmt.Sprintf("ENG-%d", i+1), "x"))
	}
	api = listFake(t, conn("c1", full...), conn("", issueNode("ENG-101", "Two")))
	r = runWith(t, opts{api: api, key: testKey}, "issue", "list", "--limit", "300", "--json")
	if calls := api.calls(); len(calls) != 2 || calls[0].Vars["first"] != 250.0 || calls[1].Vars["first"] != 50.0 || decode(t, r.out)["shown"] != 251.0 {
		t.Errorf("second page: %v", calls)
	}
}

func TestIssueListFieldsAndText(t *testing.T) {
	t.Parallel()
	evil := issueNode("ENG-1", "Ti\x1b[31mtle")
	api := listFake(t, conn("", evil))
	r := runWith(t, opts{api: api, key: testKey}, "issue", "list", "--fields", "identifier,title", "--json")
	item := decode(t, r.out)["items"].([]any)[0].(map[string]any)
	if r.code != 0 || len(item) != 2 || item["title"] != "Ti\x1b[31mtle" {
		t.Errorf("--fields: %v", item)
	}
	r = runWith(t, opts{api: api, key: testKey}, "issue", "list")
	if r.code != 0 || r.out != "ENG-1\tTodo\tAda\tTi?[31mtle\n" {
		t.Errorf("text: %q", r.out)
	}
}

func showAnswer() map[string]any {
	issue := issueNode("ENG-12", "Login times out")
	issue["description"] = "Steps:\n1. log in\n![shot](https://uploads.linear.app/a/b/c)"
	issue["children"] = nodes()
	issue["inverseRelations"] = nodes(map[string]any{"id": "r1", "type": "blocks", "issue": map[string]any{"identifier": "ENG-3", "title": "Blocker"}})
	issue["comments"] = nodes(map[string]any{"id": "c1", "body": "Seen it too", "createdAt": "2026-10-03T00:00:00.000Z",
		"user": map[string]any{"name": "Bo"}})
	return map[string]any{"issue": issue}
}

func TestIssueShowRefs(t *testing.T) {
	t.Parallel()
	api := newFake(t).on("IssueShow", showAnswer())
	for arg, want := range map[string]string{
		"eng-12": "ENG-12",
		"https://linear.app/acme/issue/ENG-12/login-times-out": "ENG-12",
		"https://linear.app/acme/issue/eng-12":                 "ENG-12",
		"0B6C1E2A-3F4D-4A5B-8C6D-7E8F9A0B1C2D":                 "0b6c1e2a-3f4d-4a5b-8c6d-7e8f9a0b1c2d",
	} {
		r := runWith(t, opts{api: api, key: testKey}, "issue", "show", arg, "--json")
		if r.code != 0 || api.last("IssueShow").Vars["id"] != want {
			t.Errorf("%s: exit %d, sent %v", arg, r.code, api.last("IssueShow").Vars["id"])
		}
	}
	n := len(api.calls())
	for _, bad := range []string{"ENG", "12", "ENG-12?x", "ENG-1#2", "https://evil.example/acme/issue/ENG-12",
		"https://linear.app/acme/project/ENG-12", "ENG-%31", "../ENG-1", ""} {
		if r := runWith(t, opts{api: api, key: testKey}, "issue", "show", bad, "--json"); r.code != exitUsage {
			t.Errorf("%q: exit %d", bad, r.code)
		}
	}
	if len(api.calls()) != n {
		t.Error("a refused ISSUE reached Linear")
	}
}

func TestIssueShow(t *testing.T) {
	t.Parallel()
	api := newFake(t).on("IssueShow", showAnswer())
	r := runWith(t, opts{api: api, key: testKey}, "issue", "show", "ENG-12", "--signed-urls", "300", "--json")
	is := decode(t, r.out)["issue"].(map[string]any)
	if r.code != 0 || is["identifier"] != "ENG-12" || len(is["comments"].([]any)) != 1 || len(is["inverseRelations"].([]any)) != 1 {
		t.Fatalf("exit %d, %q", r.code, r.out)
	}
	at, err := time.Parse(time.RFC3339, decode(t, r.out)["signedUrlsExpireAt"].(string))
	if err != nil || time.Until(at) < 295*time.Second || time.Until(at) > 301*time.Second {
		t.Errorf("signedUrlsExpireAt %v %v", at, err)
	}
	c := api.last("IssueShow")
	if c.Vars["comments"] != 20.0 || c.Vars["withComments"] != true || c.Header.Get("public-file-urls-expire-in") != "300" {
		t.Errorf("vars %v, signed %q", c.Vars, c.Header.Get("public-file-urls-expire-in"))
	}
	runWith(t, opts{api: api, key: testKey}, "issue", "show", "ENG-12", "--comments=0")
	if c := api.last("IssueShow"); c.Vars["withComments"] != false || c.Header.Get("public-file-urls-expire-in") != "" {
		t.Errorf("--comments 0: %v", c.Vars)
	}
	r = runWith(t, opts{api: api, key: testKey}, "issue", "show", "ENG-12", "--fields", "identifier,description", "--json")
	if is := decode(t, r.out)["issue"].(map[string]any); len(is) != 2 {
		t.Errorf("--fields: %v", is)
	}
	r = runWith(t, opts{api: api, key: testKey}, "issue", "show", "ENG-12")
	for _, want := range []string{"ENG-12  Login times out", "state: Todo", "Steps:\n1. log in", "— Bo", "Seen it too"} {
		if !strings.Contains(r.out, want) {
			t.Errorf("text has no %q: %q", want, r.out)
		}
	}
	if r := runWith(t, opts{api: api, key: testKey}, "issue", "show", "ENG-12", "--comments=251"); r.code != exitUsage {
		t.Errorf("--comments 251: exit %d", r.code)
	}
}

func TestIssueShowNotFound(t *testing.T) {
	t.Parallel()
	api := newFake(t).onFunc("IssueShow", linearErr("Entity not found: Issue", "INVALID_INPUT"))
	r := runWith(t, opts{api: api, key: testKey}, "issue", "show", "ENG-404", "--json")
	if e, _ := decode(t, r.out)["error"].(string); r.code != exitFailed || !strings.Contains(e, "not found: issue ENG-404") {
		t.Errorf("exit %d, %q", r.code, r.out)
	}
}

const newIssueID = "0b6c1e2a-3f4d-4a5b-8c6d-7e8f9a0b1c2d"

// resolveAnswer is a CreateResolve or UpdateResolve answer with every name
// found once.
func resolveAnswer(extra map[string]any) map[string]any {
	a := map[string]any{
		"teams":       conn("", map[string]any{"id": "team-eng", "key": "ENG", "states": conn("", map[string]any{"id": "st-doing", "name": "In Progress"})}),
		"existing":    conn(""),
		"users":       conn("", map[string]any{"id": "u-ada", "email": "ada@example.com"}),
		"viewer":      map[string]any{"id": "u-me"},
		"issueLabels": conn("", map[string]any{"id": "l-bug-ops", "name": "bug", "team": map[string]any{"id": "team-ops"}}, map[string]any{"id": "l-bug", "name": "bug", "team": map[string]any{"id": "team-eng"}}, map[string]any{"id": "l-p1", "name": "p1", "team": nil}),
		"projects":    conn("", map[string]any{"id": "p-q4", "name": "Q4"}),
		"parent":      conn("", map[string]any{"id": "id-ENG-1", "identifier": "ENG-1"}),
	}
	for k, v := range extra {
		a[k] = v
	}
	return a
}

func createFake(t *testing.T, resolve map[string]any) *fake {
	return newFake(t).on("CreateResolve", resolve).
		on("IssueCreate", map[string]any{"issueCreate": map[string]any{"success": true, "issue": issueNode("ENG-77", "New")}})
}

func TestIssueCreateNeedsAnIDToApply(t *testing.T) {
	t.Parallel()
	api := createFake(t, resolveAnswer(nil))
	r := runWith(t, opts{api: api, key: testKey}, "issue", "create", "--team", "ENG", "--title", "x", "--yes", "--json")
	if e, _ := decode(t, r.out)["error"].(string); r.code != exitUsage || !strings.Contains(e, "--id") {
		t.Errorf("exit %d, %q", r.code, r.out)
	}
	if len(api.calls()) != 0 {
		t.Error("called Linear")
	}
}

func TestIssueCreateDryRunResolvesEverything(t *testing.T) {
	t.Parallel()
	api := createFake(t, resolveAnswer(nil))
	r := runWith(t, opts{api: api, key: testKey}, "issue", "create", "--team", "eng", "--title", "Fix login",
		"--description", "It hangs", "--state", "in progress", "--assignee", "ada@example.com", "--priority", "urgent",
		"--label", "bug", "--label", "p1", "--project", "Q4", "--parent", "ENG-1", "--dry-run", "--json")
	d := decode(t, r.out)
	if r.code != 0 || d["changed"] != true || d["dryRun"] != true {
		t.Fatalf("exit %d, %q %q", r.code, r.out, r.err)
	}
	id, _ := d["id"].(string)
	if !uuidPattern.MatchString(id) || id[14] != '4' {
		t.Errorf("generated id %q", id)
	}
	want := map[string]any{"id": id, "teamId": "team-eng", "title": "Fix login", "description": "It hangs", "stateId": "st-doing",
		"assigneeId": "u-ada", "priority": 1.0, "labelIds": []any{"l-bug", "l-p1"}, "projectId": "p-q4", "parentId": "id-ENG-1"}
	if !reflect.DeepEqual(d["input"], want) {
		t.Errorf("input\n got %v\nwant %v", d["input"], want)
	}
	if slices.Contains(api.ops(), "IssueCreate") {
		t.Error("a dry run created")
	}
	v := api.last("CreateResolve").Vars
	if v["team"] != "ENG" || v["withID"] != true || v["email"] != "ada@example.com" || v["me"] != false ||
		jsonOf(v["parent"]) != `{"number":{"eq":1},"team":{"key":{"eqIgnoreCase":"ENG"}}}` {
		t.Errorf("resolve vars %v", v)
	}
}

func TestIssueCreateYesAndRerun(t *testing.T) {
	t.Parallel()
	api := createFake(t, resolveAnswer(nil))
	r := runWith(t, opts{api: api, key: testKey}, "issue", "create", "--team", "ENG", "--title", "New", "--assignee", "me",
		"--id", strings.ToUpper(newIssueID), "--yes", "--json")
	d := decode(t, r.out)
	if r.code != 0 || d["changed"] != true || d["issue"].(map[string]any)["identifier"] != "ENG-77" {
		t.Fatalf("exit %d, %q", r.code, r.out)
	}
	in := api.last("IssueCreate").Vars["input"].(map[string]any)
	if in["id"] != newIssueID || in["assigneeId"] != "u-me" || in["teamId"] != "team-eng" {
		t.Errorf("input %v", in)
	}

	// The same id again: the issue exists, nothing is created.
	api = createFake(t, resolveAnswer(map[string]any{"existing": conn("", issueNode("ENG-77", "New"))}))
	r = runWith(t, opts{api: api, key: testKey}, "issue", "create", "--team", "ENG", "--title", "New",
		"--id", newIssueID, "--yes", "--json")
	if d := decode(t, r.out); r.code != 0 || d["changed"] != false || d["issue"].(map[string]any)["identifier"] != "ENG-77" {
		t.Errorf("rerun: exit %d, %q", r.code, r.out)
	}
	if slices.Contains(api.ops(), "IssueCreate") {
		t.Error("a rerun created a second issue")
	}
}

func TestIssueCreateInputs(t *testing.T) {
	t.Parallel()
	dir := t.TempDir()
	must(t, os.WriteFile(filepath.Join(dir, "issue.json"), []byte(`{"title":"From file","estimate":3,"priority":4}`), 0o644))
	api := createFake(t, resolveAnswer(nil))
	r := runWith(t, opts{dir: dir, api: api, key: testKey, stdin: "line 1\nline 2\n"}, "issue", "create", "--team", "ENG",
		"--json-input", "issue.json", "--priority", "2", "--description-file", "-", "--dry-run", "--json")
	in := decode(t, r.out)["input"].(map[string]any)
	if r.code != 0 || in["title"] != "From file" || in["estimate"] != 3.0 || in["priority"] != 2.0 || in["description"] != "line 1\nline 2\n" {
		t.Errorf("exit %d, input %v", r.code, in)
	}
}

func TestIssueCreateRefusals(t *testing.T) {
	t.Parallel()
	dir := t.TempDir()
	must(t, os.WriteFile(filepath.Join(dir, "list.json"), []byte(`[1]`), 0o644))
	usage := [][]string{
		{"--title", "x"},  // no team
		{"--team", "ENG"}, // no title
		{"--team", "ENG", "--title", "x", "--id", "not-a-uuid"},
		{"--team", "ENG", "--title", "x", "--priority", "9"},
		{"--team", "ENG", "--title", "x", "--priority", "soon"},
		{"--team", "ENG", "--title", "x", "--assignee", "ada"},
		{"--team", "ENG", "--title", "x", "--parent", "nope"},
		{"--team", "ENG", "--title", "x", "--description", "a", "--description-file", "-"},
		{"--team", "ENG", "--json-input", "list.json"},
		{"--team", "ENG", "--title", "a\x01b"},
	}
	api := newFake(t)
	for _, args := range usage {
		r := runWith(t, opts{dir: dir, api: api, key: testKey}, append([]string{"issue", "create", "--dry-run", "--json"}, args...)...)
		if r.code != exitUsage {
			t.Errorf("%v: exit %d, %q", args, r.code, r.out)
		}
	}
	if r := runWith(t, opts{dir: dir, api: api, key: testKey}, "issue", "create", "--dry-run", "--team", "ENG", "--json-input", "missing.json"); r.code != exitFailed || !strings.Contains(r.err, "no such file: missing.json") {
		t.Errorf("missing file: exit %d, %q", r.code, r.err)
	}
	if len(api.calls()) != 0 {
		t.Errorf("called Linear: %v", api.ops())
	}

	failed := []struct {
		args    []string
		resolve map[string]any
		want    string
	}{
		{[]string{"--team", "NOPE"}, map[string]any{"teams": conn("")}, "no team with key NOPE"},
		{[]string{"--state", "Doing"}, map[string]any{"teams": conn("", map[string]any{"id": "team-eng", "key": "ENG", "states": conn("")})}, `no workflow state named "Doing"; see 'linearctl state list'`},
		{[]string{"--assignee", "zed@example.com"}, map[string]any{"users": conn("")}, `no user with email named "zed@example.com"`},
		{[]string{"--label", "p2"}, nil, `no label named "p2"`},
		{[]string{"--project", "Q4"}, map[string]any{"projects": conn("", map[string]any{"id": "a", "name": "Q4"}, map[string]any{"id": "b", "name": "Q4"})}, `2 projects are named "Q4"`},
		{[]string{"--parent", "ENG-9"}, map[string]any{"parent": conn("")}, "not found: parent issue ENG-9"},
	}
	for _, c := range failed {
		api := createFake(t, resolveAnswer(c.resolve))
		args := append([]string{"issue", "create", "--team", "ENG", "--title", "x", "--dry-run", "--json"}, c.args...)
		r := runWith(t, opts{api: api, key: testKey}, args...)
		if e, _ := decode(t, r.out)["error"].(string); r.code != exitFailed || !strings.Contains(e, c.want) {
			t.Errorf("%v: exit %d, %q", c.args, r.code, r.out)
		}
	}
}

// currentIssue is an UpdateResolve issue: In Progress, assigned to Ada,
// labelled bug.
func currentIssue() map[string]any {
	return map[string]any{
		"id": "id-ENG-5", "identifier": "ENG-5", "title": "Old title", "description": nil, "priority": 3,
		"updatedAt": "2026-10-02T08:00:00.000Z",
		"team":     map[string]any{"id": "team-eng", "key": "ENG", "states": conn("", map[string]any{"id": "st-doing", "name": "In Progress"})},
		"state":    map[string]any{"id": "st-doing", "name": "In Progress"},
		"assignee": map[string]any{"id": "u-ada", "email": "ada@example.com"},
		"project":  nil, "parent": nil,
		"labels": conn("", map[string]any{"id": "l-bug", "name": "bug"}),
	}
}

func updateFake(t *testing.T) *fake {
	return newFake(t).on("UpdateResolve", resolveAnswer(map[string]any{"issue": currentIssue()})).
		on("IssueUpdate", map[string]any{"issueUpdate": map[string]any{"success": true, "issue": issueNode("ENG-5", "New title")}})
}

func TestIssueUpdateSendsOnlyWhatDiffers(t *testing.T) {
	t.Parallel()
	cases := []struct {
		args []string
		want string // the input sent, as JSON; "" for none
	}{
		{[]string{"--state", "In Progress", "--assignee", "ada@example.com", "--priority", "medium", "--title", "Old title"}, ""},
		{[]string{"--add-label", "bug", "--remove-label", "p1"}, ""},
		{[]string{"--label", "bug"}, ""},
		{[]string{"--title", "New title"}, `{"title":"New title"}`},
		{[]string{"--assignee", "none"}, `{"assigneeId":null}`},
		{[]string{"--parent", "none"}, ""}, // it has no parent
		{[]string{"--parent", "ENG-1"}, `{"parentId":"id-ENG-1"}`},
		{[]string{"--assignee", "me", "--priority", "1"}, `{"assigneeId":"u-me","priority":1}`},
		{[]string{"--add-label", "p1", "--remove-label", "bug"}, `{"addedLabelIds":["l-p1"],"removedLabelIds":["l-bug"]}`},
		{[]string{"--label", "p1"}, `{"labelIds":["l-p1"]}`},
		{[]string{"--description", "Now with steps"}, `{"description":"Now with steps"}`},
		{[]string{"--project", "Q4", "--parent", "ENG-1"}, `{"parentId":"id-ENG-1","projectId":"p-q4"}`},
	}
	for _, c := range cases {
		api := updateFake(t)
		r := runWith(t, opts{api: api, key: testKey}, append([]string{"issue", "update", "ENG-5", "--yes", "--json"}, c.args...)...)
		d := decode(t, r.out)
		if r.code != 0 || d["changed"] != (c.want != "") {
			t.Errorf("%v: exit %d, %q", c.args, r.code, r.out)
			continue
		}
		sent := slices.Contains(api.ops(), "IssueUpdate")
		if sent != (c.want != "") || (sent && jsonOf(api.last("IssueUpdate").Vars["input"]) != c.want) {
			t.Errorf("%v: sent %v %v, want %s", c.args, sent, api.calls()[len(api.calls())-1].Vars["input"], c.want)
		}
	}
}

func TestIssueUpdateDryRunAndRefusals(t *testing.T) {
	t.Parallel()
	api := updateFake(t)
	r := runWith(t, opts{api: api, key: testKey}, "issue", "update", "ENG-5", "--title", "New title", "--dry-run", "--json")
	if d := decode(t, r.out); r.code != 0 || d["changed"] != true || slices.Contains(api.ops(), "IssueUpdate") {
		t.Errorf("dry run: exit %d, %q, ops %v", r.code, r.out, api.ops())
	}
	r = runWith(t, opts{api: api, key: testKey}, "issue", "update", "ENG-5", "--label", "bug", "--add-label", "p1", "--dry-run")
	if r.code != exitUsage {
		t.Errorf("--label with --add-label: exit %d", r.code)
	}
	api = newFake(t).onFunc("UpdateResolve", linearErr("Entity not found: Issue", "INVALID_INPUT"))
	r = runWith(t, opts{api: api, key: testKey}, "issue", "update", "ENG-404", "--title", "x", "--yes", "--json")
	if e, _ := decode(t, r.out)["error"].(string); r.code != exitFailed || !strings.Contains(e, "not found: issue ENG-404") {
		t.Errorf("missing: exit %d, %q", r.code, r.out)
	}
}

func TestIssueArchive(t *testing.T) {
	t.Parallel()
	archived := func(at any) *fake {
		return newFake(t).on("IssueArchived", map[string]any{"issue": map[string]any{"id": "id-ENG-5", "identifier": "ENG-5", "archivedAt": at}}).
			on("IssueArchive", map[string]any{"issueArchive": map[string]any{"success": true}})
	}
	api := archived(nil)
	r := runWith(t, opts{api: api, key: testKey}, "issue", "archive", "ENG-5", "--dry-run", "--json")
	if d := decode(t, r.out); r.code != 0 || d["changed"] != true || slices.Contains(api.ops(), "IssueArchive") {
		t.Errorf("dry run: exit %d, %q", r.code, r.out)
	}
	r = runWith(t, opts{api: api, key: testKey}, "issue", "archive", "ENG-5", "--yes", "--json")
	if r.code != 0 || api.last("IssueArchive").Vars["id"] != "id-ENG-5" {
		t.Errorf("yes: exit %d, %q", r.code, r.out)
	}
	api = archived("2026-10-01T00:00:00.000Z")
	r = runWith(t, opts{api: api, key: testKey}, "issue", "archive", "ENG-5", "--yes", "--json")
	if d := decode(t, r.out); r.code != 0 || d["changed"] != false || slices.Contains(api.ops(), "IssueArchive") {
		t.Errorf("rerun: exit %d, %q", r.code, r.out)
	}
}

// Agents write gh's "issue view ENG-123 --comments" first; both work.
func TestIssueViewAndBareComments(t *testing.T) {
	t.Parallel()
	api := newFake(t).on("IssueShow", showAnswer())
	r := runWith(t, opts{api: api, key: testKey}, "issue", "view", "ENG-12", "--comments", "--json")
	if r.code != 0 || api.last("IssueShow").Vars["comments"] != 20.0 {
		t.Fatalf("view --comments: exit %d, %q %q", r.code, r.out, r.err)
	}
	runWith(t, opts{api: api, key: testKey}, "issue", "view", "ENG-12", "--comments=5")
	if v := api.last("IssueShow").Vars; v["comments"] != 5.0 {
		t.Errorf("--comments=5: %v", v)
	}
	n := len(api.calls())
	r = runWith(t, opts{api: api, key: testKey}, "issue", "show", "ENG-12", "--comments", "5", "--json")
	if e, _ := decode(t, r.out)["error"].(string); r.code != exitUsage || !strings.Contains(e, "--comments=5") || len(api.calls()) != n {
		t.Errorf("--comments 5: exit %d, %q", r.code, r.out)
	}
	var d struct {
		Commands []struct {
			Name     string
			Commands []struct {
				Name    string
				Aliases []string
			}
		}
	}
	must(t, json.Unmarshal([]byte(run(t, "describe").out), &d))
	for _, c := range d.Commands {
		for _, s := range c.Commands {
			if want := c.Name == "issue" && s.Name == "show"; want != slices.Contains(s.Aliases, "view") {
				t.Errorf("%s %s: aliases %v", c.Name, s.Name, s.Aliases)
			}
		}
	}
}

// Nested objects carry ids (and colors), so callers can join them reliably.
func TestIssueRowNestsIDs(t *testing.T) {
	t.Parallel()
	for _, want := range []string{"state { id name type color }", "assignee { id name email }", "team { id key }",
		"project { id name }", "cycle { id number }", "labels(first: 20) { nodes { id name color } }"} {
		if !strings.Contains(issueRow, want) {
			t.Errorf("issueRow lacks %q", want)
		}
	}
}

func TestIssueUnarchive(t *testing.T) {
	t.Parallel()
	archived := func(at any) *fake {
		return newFake(t).on("IssueArchived", map[string]any{"issue": map[string]any{"id": "id-ENG-5", "identifier": "ENG-5", "archivedAt": at}}).
			on("IssueUnarchive", map[string]any{"issueUnarchive": map[string]any{"success": true}})
	}
	api := archived("2026-10-01T00:00:00.000Z")
	r := runWith(t, opts{api: api, key: testKey}, "issue", "unarchive", "ENG-5", "--dry-run", "--json")
	if d := decode(t, r.out); r.code != 0 || d["changed"] != true || slices.Contains(api.ops(), "IssueUnarchive") {
		t.Errorf("dry run: exit %d, %q", r.code, r.out)
	}
	r = runWith(t, opts{api: api, key: testKey}, "issue", "unarchive", "ENG-5", "--yes")
	if r.code != 0 || api.last("IssueUnarchive").Vars["id"] != "id-ENG-5" || r.out != "unarchived ENG-5\n" {
		t.Errorf("yes: exit %d, %q", r.code, r.out)
	}
	api = archived(nil)
	r = runWith(t, opts{api: api, key: testKey}, "issue", "unarchive", "ENG-5", "--yes", "--json")
	if d := decode(t, r.out); r.code != 0 || d["changed"] != false || slices.Contains(api.ops(), "IssueUnarchive") {
		t.Errorf("not archived: exit %d, %q", r.code, r.out)
	}
}

func TestIssueListByID(t *testing.T) {
	t.Parallel()
	api := listFake(t, conn("", issueNode("ENG-1", "One"), issueNode("ENG-2", "Two")))
	r := runWith(t, opts{api: api, key: testKey}, "issue", "list", "--id", "eng-1", "--id", "ENG-2", "--id", "ENG-1", "--json")
	v := api.last("IssueList").Vars
	if r.code != 0 || jsonOf(v["filter"]) != `{"id":{"in":["ENG-1","ENG-2"]}}` || v["first"] != float64(pageSize) {
		t.Errorf("exit %d, vars %v", r.code, v)
	}
	r = runWith(t, opts{api: api, key: testKey}, "issue", "list", "--id", "ENG-1", "--id", "ENG-9", "--team", "ENG", "--json")
	d := decode(t, r.out)
	p, _ := d["problems"].([]any)
	if r.code != exitPartial || d["shown"] != 2.0 || len(p) != 1 || p[0].(map[string]any)["input"] != "ENG-9" {
		t.Errorf("missing: exit %d, %q", r.code, r.out)
	}
	if got := jsonOf(api.last("IssueList").Vars["filter"]); got != `{"id":{"in":["ENG-1","ENG-9"]},"team":{"key":{"eq":"ENG"}}}` {
		t.Errorf("with --team: %s", got)
	}
	if r := runWith(t, opts{api: api, key: testKey}, "issue", "list", "--id", "nope"); r.code != exitUsage {
		t.Errorf("--id nope: exit %d", r.code)
	}
}

func TestIssueUpdateDetachesFromItsParent(t *testing.T) {
	t.Parallel()
	is := currentIssue()
	is["parent"] = map[string]any{"id": "id-ENG-1", "identifier": "ENG-1"}
	api := newFake(t).on("UpdateResolve", resolveAnswer(map[string]any{"issue": is}))
	r := runWith(t, opts{api: api, key: testKey}, "issue", "update", "ENG-5", "--parent", "none", "--dry-run", "--json")
	if d := decode(t, r.out); r.code != 0 || jsonOf(d["input"]) != `{"parentId":null}` || api.last("UpdateResolve").Vars["withParent"] != false {
		t.Errorf("exit %d, %q", r.code, r.out)
	}
}

func TestIssueUpdateIfUpdatedAt(t *testing.T) {
	t.Parallel()
	for _, c := range []struct {
		since string
		code  int
	}{
		{"2026-10-02T08:00:00Z", 0},
		{"2026-10-02T16:00:00+08:00", 0}, // the same instant
		{"2026-10-01T00:00:00Z", exitConflict},
		{"yesterday", exitUsage},
	} {
		api := updateFake(t)
		r := runWith(t, opts{api: api, key: testKey}, "issue", "update", "ENG-5", "--title", "New title", "--if-updated-at", c.since, "--yes", "--json")
		if r.code != c.code {
			t.Errorf("%s: exit %d, %q", c.since, r.code, r.out)
		}
		if sent := slices.Contains(api.ops(), "IssueUpdate"); sent != (c.code == 0) {
			t.Errorf("%s: sent %v", c.since, sent)
		}
		if c.code == exitConflict && !strings.Contains(decode(t, r.out)["error"].(string), "changed at 2026-10-02T08:00:00.000Z") {
			t.Errorf("%s: %q", c.since, r.out)
		}
	}
}
