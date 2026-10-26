package main

import (
	"strings"
	"testing"
)

func TestLookupLists(t *testing.T) {
	t.Parallel()
	cases := []struct {
		args       []string
		op, conn   string
		nodes      []any
		wantFilter string // "" for none
		wantText   string
	}{
		{[]string{"team", "list"}, "Teams", "teams",
			[]any{map[string]any{"id": "t1", "key": "ENG", "name": "Engineering"}}, "", "ENG\tEngineering\n"},
		{[]string{"state", "list", "--team", "eng"}, "States", "workflowStates",
			[]any{
				map[string]any{"name": "Done", "type": "completed", "position": 3.0, "team": map[string]any{"key": "ENG"}},
				map[string]any{"name": "Todo", "type": "unstarted", "position": 1.0, "team": map[string]any{"key": "ENG"}},
				map[string]any{"name": "Backlog", "type": "backlog", "position": 0.0, "team": map[string]any{"key": "APP"}},
			},
			`{"team":{"key":{"eqIgnoreCase":"ENG"}}}`, "APP\tBacklog\tbacklog\nENG\tTodo\tunstarted\nENG\tDone\tcompleted\n"},
		{[]string{"label", "list", "--team", "ENG"}, "Labels", "issueLabels",
			[]any{map[string]any{"name": "p1", "team": nil}, map[string]any{"name": "bug", "team": map[string]any{"key": "ENG"}}},
			`{"or":[{"team":{"key":{"eqIgnoreCase":"ENG"}}},{"team":{"null":true}}]}`, "bug\tENG\t-\np1\t-\t-\n"},
		{[]string{"user", "list", "--active"}, "Users", "users",
			[]any{map[string]any{"name": "Ada", "email": "ada@example.com"}}, `{"active":{"eq":true}}`, "ada@example.com\tAda\n"},
		{[]string{"user", "list"}, "Users", "users", nil, "", ""},
		{[]string{"project", "list", "--team", "ENG", "--status", "started", "--status", "planned"}, "Projects", "projects",
			[]any{map[string]any{"name": "Q4", "status": map[string]any{"name": "In Progress"}, "lead": nil}},
			`{"accessibleTeams":{"some":{"key":{"eqIgnoreCase":"ENG"}}},"status":{"type":{"in":["started","planned"]}}}`, "Q4\tIn Progress\t-\n"},
		{[]string{"cycle", "list", "--team", "ENG"}, "Cycles", "cycles",
			[]any{map[string]any{"number": 12.0, "startsAt": "2026-10-20", "endsAt": "2026-11-03", "team": map[string]any{"key": "ENG"}}},
			`{"team":{"key":{"eqIgnoreCase":"ENG"}}}`, "ENG\t12\t2026-10-20\t2026-11-03\n"},
	}
	for _, c := range cases {
		api := newFake(t).on(c.op, map[string]any{c.conn: conn("", c.nodes...)})
		r := runWith(t, opts{api: api, key: testKey}, append(c.args, "--json")...)
		if d := decode(t, r.out); r.code != 0 || d["shown"] != float64(len(c.nodes)) {
			t.Errorf("%v: exit %d, %q %q", c.args, r.code, r.out, r.err)
			continue
		}
		v := api.last(c.op).Vars
		if got := jsonOf(v["filter"]); (c.wantFilter == "" && v["filter"] != nil) || (c.wantFilter != "" && got != c.wantFilter) {
			t.Errorf("%v: filter %s, want %s", c.args, got, c.wantFilter)
		}
		if v["first"] != 50.0 {
			t.Errorf("%v: first %v", c.args, v["first"])
		}
		if r := runWith(t, opts{api: api, key: testKey}, c.args...); r.out != c.wantText {
			t.Errorf("%v: text %q, want %q", c.args, r.out, c.wantText)
		}
	}
}

func TestLookupFieldsAndRefusals(t *testing.T) {
	t.Parallel()
	api := newFake(t).on("Teams", map[string]any{"teams": conn("", map[string]any{"id": "t1", "key": "ENG", "name": "Engineering"})})
	r := runWith(t, opts{api: api, key: testKey}, "team", "list", "--fields", "key", "--json")
	if item := decode(t, r.out)["items"].([]any)[0].(map[string]any); r.code != 0 || len(item) != 1 || item["key"] != "ENG" {
		t.Errorf("--fields: %q", r.out)
	}
	n := len(api.calls())
	for _, args := range [][]string{
		{"team", "list", "--fields", "email"}, {"project", "list", "--status", "done"}, {"state", "list", "--team", "a b"},
		{"team", "list", "--team", "ENG"}, {"user", "list", "--status", "x"},
	} {
		if r := runWith(t, opts{api: api, key: testKey}, args...); r.code != exitUsage {
			t.Errorf("%v: exit %d", args, r.code)
		}
	}
	if len(api.calls()) != n {
		t.Error("a refused invocation called Linear")
	}
	// Another list's help names its fields.
	if r := run(t, "cycle", "list", "--help"); !strings.Contains(r.out, "Fields: id, number, name") {
		t.Errorf("help: %q", r.out)
	}
}

func TestNotificationList(t *testing.T) {
	t.Parallel()
	api := newFake(t).on("Notifications", map[string]any{"notifications": conn("", map[string]any{"id": "n1", "type": "issueAssignedToYou",
		"title": "Fix login", "createdAt": "2026-10-26T00:00:00.000Z", "readAt": nil, "issue": map[string]any{"identifier": "ENG-1"}})})
	r := runWith(t, opts{api: api, key: testKey}, "notification", "list", "--type", "issueAssignedToYou")
	if r.code != 0 || r.out != "2026-10-26T00:00:00.000Z\tissueAssignedToYou\tENG-1\tFix login\n" ||
		jsonOf(api.last("Notifications").Vars["filter"]) != `{"type":{"eq":"issueAssignedToYou"}}` {
		t.Errorf("exit %d, %q, %v", r.code, r.out, api.last("Notifications").Vars)
	}
}
