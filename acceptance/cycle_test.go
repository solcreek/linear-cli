package acceptance

import (
	"slices"
	"strings"
	"testing"
)

// cycleNode is a cycle as CycleShow returns it, with Linear's fields.
func cycleNode(number float64, team string) map[string]any {
	return map[string]any{
		"id": "cy-" + team, "number": number, "name": nil, "description": "Ship checkout v2",
		"startsAt": "2026-10-19T00:00:00.000Z", "endsAt": "2026-11-02T00:00:00.000Z", "completedAt": nil,
		"isActive": true, "isNext": false, "isPrevious": false, "isPast": false, "isFuture": false,
		"progress": 0.42, "team": map[string]any{"id": "t-" + team, "key": team},
		"createdAt": "2026-10-01T00:00:00.000Z", "updatedAt": "2026-10-20T00:00:00.000Z", "archivedAt": nil, "autoArchivedAt": nil,
		"currentProgress": map[string]any{"completed": map[string]any{"count": 3, "estimate": 5}},
		"progressHistory": map[string]any{}, "scopeHistory": []any{8.0, 10.0}, "completedScopeHistory": []any{0.0, 3.0},
		"inProgressScopeHistory": []any{2.0, 4.0}, "issueCountHistory": []any{5.0, 6.0}, "completedIssueCountHistory": []any{0.0, 2.0},
		"inheritedFrom":              nil,
		"issues":                     conn("c-issues", issueNode("ENG-1", "One")),
		"uncompletedIssuesUponClose": conn(""),
	}
}

// cycleFake answers CycleShow as Linear would: matches holds up to two
// cycles (id and team), the detail the first one.
func cycleFake(t *testing.T, found ...any) *fake {
	matches, detail := []any{}, []any{}
	for i, f := range found {
		c := f.(map[string]any)
		matches = append(matches, map[string]any{"id": c["id"], "team": map[string]any{"key": c["team"].(map[string]any)["key"]}})
		if i == 0 {
			detail = append(detail, c)
		}
	}
	return newFake(t).on("CycleShow", map[string]any{"matches": conn("", matches...), "cycles": conn("", detail...)})
}

func TestCycleShow(t *testing.T) {
	t.Parallel()
	api := cycleFake(t, cycleNode(23, "ENG"))
	r := runWith(t, opts{api: api, key: testKey}, "cycle", "show", "current", "--team", "eng", "--json")
	c, _ := decode(t, r.out)["cycle"].(map[string]any)
	if r.code != 0 || c["progress"] != 0.42 || c["number"] != 23.0 || len(c["scopeHistory"].([]any)) != 2 {
		t.Fatalf("exit %d, %q %q", r.code, r.out, r.err)
	}
	// Linear's fields pass through under Linear's names, nothing computed.
	for _, k := range []string{"currentProgress", "progressHistory", "completedScopeHistory", "inProgressScopeHistory",
		"issueCountHistory", "completedIssueCountHistory", "isPrevious", "isPast", "isFuture", "completedAt", "inheritedFrom"} {
		if _, ok := c[k]; !ok {
			t.Errorf("no %s", k)
		}
	}
	is := c["issues"].(map[string]any)
	if is["truncated"] != true || is["nextCursor"] != "c-issues" || is["shown"] != 1.0 || is["total"] != nil {
		t.Errorf("issues %v", is)
	}
	if u := c["uncompletedIssuesUponClose"].(map[string]any); u["shown"] != 0.0 || u["nextCursor"] != nil {
		t.Errorf("uncompleted %v", u)
	}
	if !strings.Contains(r.err, "--cursor c-issues") {
		t.Errorf("stderr %q", r.err)
	}
	v := api.last("CycleShow").Vars
	if jsonOf(v["filter"]) != `{"isActive":{"eq":true},"team":{"key":{"eqIgnoreCase":"ENG"}}}` ||
		v["issues"] != 50.0 || v["withIssues"] != true || v["uncompleted"] != 50.0 || v["after"] != nil {
		t.Errorf("vars %v", v)
	}
}

func TestCycleShowRefs(t *testing.T) {
	t.Parallel()
	api := cycleFake(t, cycleNode(23, "ENG"))
	for word, want := range map[string]string{
		"next":                                 `{"isNext":{"eq":true}}`,
		"previous":                             `{"isPrevious":{"eq":true}}`,
		"23":                                   `{"number":{"eq":23}}`,
		"5F1E2A3B-4C5D-4E6F-8A9B-0C1D2E3F4A5B": `{"id":{"eq":"5f1e2a3b-4c5d-4e6f-8a9b-0c1d2e3f4a5b"}}`,
	} {
		r := runWith(t, opts{api: api, key: testKey}, "cycle", "show", word, "--json")
		if got := jsonOf(api.last("CycleShow").Vars["filter"]); r.code != 0 || got != want {
			t.Errorf("%s: exit %d, filter %s", word, r.code, got)
		}
	}
	runWith(t, opts{api: api, key: testKey}, "cycle", "show", "23", "--team", "ENG", "--issues", "0", "--uncompleted", "0", "--cursor", "c9")
	if v := api.last("CycleShow").Vars; v["withIssues"] != false || v["withUncompleted"] != false || v["after"] != "c9" {
		t.Errorf("--issues 0 --cursor: %v", v)
	}
	r := runWith(t, opts{api: api, key: testKey}, "cycle", "show", "current", "--fields", "number,progress", "--json")
	if c := decode(t, r.out)["cycle"].(map[string]any); len(c) != 2 {
		t.Errorf("--fields: %v", c)
	}
	n := len(api.calls())
	for _, args := range [][]string{{"last"}, {"-1"}, {"current", "--issues", "251"}, {"current", "--fields", "velocity"}, {""}, {"1234567890"}} {
		if r := runWith(t, opts{api: api, key: testKey}, append([]string{"cycle", "show"}, args...)...); r.code != exitUsage {
			t.Errorf("%v: exit %d", args, r.code)
		}
	}
	if len(api.calls()) != n {
		t.Error("a refused CYCLE reached Linear")
	}
}

func TestCycleShowNotFoundAndAmbiguous(t *testing.T) {
	t.Parallel()
	r := runWith(t, opts{api: cycleFake(t), key: testKey}, "cycle", "show", "next", "--team", "ENG", "--json")
	if e, _ := decode(t, r.out)["error"].(string); r.code != exitFailed || !strings.Contains(e, "not found: cycle next") {
		t.Errorf("none: exit %d, %q", r.code, r.out)
	}
	r = runWith(t, opts{api: cycleFake(t, cycleNode(23, "ENG"), cycleNode(23, "WEB")), key: testKey}, "cycle", "show", "current", "--json")
	if e, _ := decode(t, r.out)["error"].(string); r.code != exitFailed || !strings.Contains(e, "teams ENG and WEB") || !strings.Contains(e, "--team") {
		t.Errorf("two: exit %d, %q", r.code, r.out)
	}
}

func TestCycleShowText(t *testing.T) {
	t.Parallel()
	r := runWith(t, opts{api: cycleFake(t, cycleNode(23, "ENG")), key: testKey}, "cycle", "show", "current")
	for _, want := range []string{"Cycle 23  ENG  (current)", "progress 0.42", "Ship checkout v2", "ENG-1\tTodo"} {
		if !strings.Contains(r.out, want) {
			t.Errorf("text has no %q: %q", want, r.out)
		}
	}
}

func TestCycleShowInDemo(t *testing.T) {
	t.Parallel()
	for _, word := range []string{"current", "previous", "next"} {
		r := runWith(t, opts{env: []string{"LINEARCTL_DEMO=1"}}, "cycle", "show", word, "--json")
		c, _ := decode(t, r.out)["cycle"].(map[string]any)
		if r.code != 0 || c["progress"] == nil || c["scopeHistory"] == nil || c["issues"] == nil {
			t.Errorf("%s: exit %d, %q", word, r.code, r.out)
		}
		flag := map[string]string{"current": "isActive", "previous": "isPrevious", "next": "isNext"}[word]
		if c[flag] != true {
			t.Errorf("%s: %s is %v", word, flag, c[flag])
		}
	}
	keys := []string{}
	r := runWith(t, opts{env: []string{"LINEARCTL_DEMO=1"}}, "cycle", "show", "current", "--json")
	for k := range decode(t, r.out)["cycle"].(map[string]any) {
		keys = append(keys, k)
	}
	slices.Sort(keys)
	if len(keys) != 28 {
		t.Errorf("%d fields in demo: %v", len(keys), keys)
	}
}

func TestCycleListFilters(t *testing.T) {
	t.Parallel()
	cases := []struct {
		args []string
		want string
	}{
		{[]string{"--team", "eng"}, `{"team":{"key":{"eqIgnoreCase":"ENG"}}}`},
		{[]string{"--active"}, `{"isActive":{"eq":true}}`},
		{[]string{"--past", "--future"}, `{"or":[{"isPast":{"eq":true}},{"isFuture":{"eq":true}}]}`},
		{[]string{"--team", "ENG", "--previous", "--starts-since", "2026-10-01", "--ends-before", "2026-11-01T00:00:00Z"},
			`{"and":[{"team":{"key":{"eqIgnoreCase":"ENG"}}},{"isPrevious":{"eq":true}},{"startsAt":{"gte":"2026-10-01T00:00:00Z"}},{"endsAt":{"lte":"2026-11-01T00:00:00Z"}}]}`},
	}
	for _, c := range cases {
		api := newFake(t).on("Cycles", map[string]any{"cycles": conn("")})
		r := runWith(t, opts{api: api, key: testKey}, append([]string{"cycle", "list", "--json"}, c.args...)...)
		if got := jsonOf(api.last("Cycles").Vars["filter"]); r.code != 0 || got != c.want {
			t.Errorf("%v: exit %d, filter %s", c.args, r.code, got)
		}
	}
	for _, args := range [][]string{{"--starts-since", "soon"}, {"--ends-before", ""}, {"--fields", "velocity"}} {
		if r := runWith(t, opts{key: testKey}, append([]string{"cycle", "list"}, args...)...); r.code != exitUsage {
			t.Errorf("%v: exit %d", args, r.code)
		}
	}
}

const newCycleID = "0b6c1e2a-3f4d-4a5b-8c6d-7e8f9a0b1c2d"

func cycleRow(id string, archived any) map[string]any {
	c := cycleNode(24, "ENG")
	c["id"], c["name"], c["archivedAt"] = id, "Sprint", archived
	for _, k := range []string{"issues", "uncompletedIssuesUponClose"} {
		delete(c, k)
	}
	return c
}

func TestCycleCreate(t *testing.T) {
	t.Parallel()
	resolve := func(existing ...any) *fake {
		return newFake(t).on("CycleCreateResolve", map[string]any{"teams": conn("", map[string]any{"id": "t-ENG", "key": "ENG"}), "existing": conn("", existing...)}).
			on("CycleCreate", map[string]any{"cycleCreate": map[string]any{"success": true, "cycle": cycleRow(newCycleID, nil)}})
	}
	api := resolve()
	r := runWith(t, opts{api: api, key: testKey}, "cycle", "create", "--team", "eng", "--starts-at", "2026-11-02", "--ends-at", "2026-11-16T12:00:00+08:00",
		"--name", "Sprint", "--dry-run", "--json")
	d := decode(t, r.out)
	in, _ := d["input"].(map[string]any)
	if r.code != 0 || in["teamId"] != "t-ENG" || in["startsAt"] != "2026-11-02T00:00:00Z" || in["endsAt"] != "2026-11-16T04:00:00Z" || in["name"] != "Sprint" || in["id"] != d["id"] {
		t.Fatalf("dry run: exit %d, %q", r.code, r.out)
	}
	if slices.Contains(api.ops(), "CycleCreate") {
		t.Error("a dry run created")
	}
	r = runWith(t, opts{api: api, key: testKey}, "cycle", "create", "--team", "ENG", "--starts-at", "2026-11-02", "--ends-at", "2026-11-16", "--id", newCycleID, "--yes", "--json")
	if in := api.last("CycleCreate").Vars["input"].(map[string]any); r.code != 0 || in["id"] != newCycleID || decode(t, r.out)["cycle"] == nil {
		t.Errorf("yes: exit %d, %q", r.code, r.out)
	}
	api = resolve(cycleRow(newCycleID, nil))
	r = runWith(t, opts{api: api, key: testKey}, "cycle", "create", "--team", "ENG", "--starts-at", "2026-11-02", "--ends-at", "2026-11-16", "--id", newCycleID, "--yes", "--json")
	if r.code != 0 || decode(t, r.out)["changed"] != false || slices.Contains(api.ops(), "CycleCreate") {
		t.Errorf("rerun: exit %d, %q", r.code, r.out)
	}
	n := len(api.calls())
	for _, args := range [][]string{
		{"--team", "ENG", "--starts-at", "2026-11-02", "--ends-at", "2026-11-16", "--yes"}, // no --id
		{"--team", "ENG", "--ends-at", "2026-11-16", "--dry-run"},
		{"--team", "ENG", "--starts-at", "2026-11-16", "--ends-at", "2026-11-02", "--dry-run"},
		{"--team", "ENG", "--starts-at", "soon", "--ends-at", "2026-11-16", "--dry-run"},
		{"--starts-at", "2026-11-02", "--ends-at", "2026-11-16", "--dry-run"},
	} {
		if r := runWith(t, opts{api: api, key: testKey}, append([]string{"cycle", "create"}, args...)...); r.code != exitUsage {
			t.Errorf("%v: exit %d, %q", args, r.code, r.err)
		}
	}
	if len(api.calls()) != n {
		t.Error("a refused create reached Linear")
	}
}

func TestCycleUpdateAndArchive(t *testing.T) {
	t.Parallel()
	find := func(cycles ...any) *fake {
		return newFake(t).on("CycleFind", map[string]any{"cycles": conn("", cycles...)}).
			on("CycleUpdate", map[string]any{"cycleUpdate": map[string]any{"success": true, "cycle": cycleRow("cy", nil)}}).
			on("CycleArchive", map[string]any{"cycleArchive": map[string]any{"success": true}})
	}
	for _, c := range []struct {
		args []string
		want string
	}{
		{[]string{"--name", "Sprint", "--starts-at", "2026-10-19", "--ends-at", "2026-11-02T08:00:00+08:00"}, ""}, // all as they are
		{[]string{"--name", "Checkout", "--ends-at", "2026-11-05"}, `{"endsAt":"2026-11-05T00:00:00Z","name":"Checkout"}`},
		{[]string{"--description", "Ship checkout v2"}, ""},
		{[]string{"--description", "Ship it"}, `{"description":"Ship it"}`},
		{[]string{"--completed-at", "2026-10-30T10:00:00Z"}, `{"completedAt":"2026-10-30T10:00:00Z"}`},
	} {
		api := find(cycleRow("cy", nil))
		r := runWith(t, opts{api: api, key: testKey}, append([]string{"cycle", "update", "current", "--team", "ENG", "--yes", "--json"}, c.args...)...)
		d := decode(t, r.out)
		if r.code != 0 || d["changed"] != (c.want != "") {
			t.Errorf("%v: exit %d, %q", c.args, r.code, r.out)
			continue
		}
		if sent := slices.Contains(api.ops(), "CycleUpdate"); sent != (c.want != "") || (sent && jsonOf(api.last("CycleUpdate").Vars["input"]) != c.want) {
			t.Errorf("%v: sent %v %v", c.args, sent, d["input"])
		}
	}
	api := find(cycleRow("cy", nil))
	if r := runWith(t, opts{api: api, key: testKey}, "cycle", "update", "current", "--dry-run"); r.code != exitUsage {
		t.Errorf("nothing to change: exit %d", r.code)
	}
	r := runWith(t, opts{api: find(cycleRow("a", nil), cycleNode(24, "WEB")), key: testKey}, "cycle", "update", "24", "--name", "x", "--dry-run", "--json")
	if e, _ := decode(t, r.out)["error"].(string); r.code != exitFailed || !strings.Contains(e, "pass --team") {
		t.Errorf("ambiguous: exit %d, %q", r.code, r.out)
	}
	api = find(cycleRow("cy", nil))
	r = runWith(t, opts{api: api, key: testKey}, "cycle", "archive", "previous", "--team", "ENG", "--yes", "--json")
	if r.code != 0 || decode(t, r.out)["changed"] != true || api.last("CycleArchive").Vars["id"] != "cy" {
		t.Errorf("archive: exit %d, %q", r.code, r.out)
	}
	api = find(cycleRow("cy", "2026-10-25T00:00:00.000Z"))
	r = runWith(t, opts{api: api, key: testKey}, "cycle", "archive", "previous", "--yes", "--json")
	if r.code != 0 || decode(t, r.out)["changed"] != false || slices.Contains(api.ops(), "CycleArchive") {
		t.Errorf("already archived: exit %d, %q", r.code, r.out)
	}
}

func TestCycleShowAllIssues(t *testing.T) {
	t.Parallel()
	var after []any
	api := cycleFake(t, cycleNode(23, "ENG")).onFunc("CycleIssues", func(v map[string]any) (int, any, map[string]string) {
		after = append(after, v["after"])
		page := conn("c-2", issueNode("ENG-2", "Two"))
		if len(after) == 2 {
			page = conn("", issueNode("ENG-3", "Three"))
		}
		return 200, map[string]any{"data": map[string]any{"cycle": map[string]any{"issues": page}}}, nil
	})
	r := runWith(t, opts{api: api, key: testKey}, "cycle", "show", "current", "--team", "ENG", "--all-issues", "--json")
	is, _ := decode(t, r.out)["cycle"].(map[string]any)["issues"].(map[string]any)
	if r.code != 0 || is["shown"] != 3.0 || is["truncated"] != false || is["nextCursor"] != nil || r.err != "" {
		t.Fatalf("exit %d, issues %v, stderr %q", r.code, is, r.err)
	}
	if v := api.last("CycleShow").Vars; v["issues"] != 250.0 || v["withIssues"] != true {
		t.Errorf("first page vars %v", v)
	}
	if jsonOf(after) != `["c-issues","c-2"]` || api.last("CycleIssues").Vars["id"] != "cy-ENG" {
		t.Errorf("follow-ups after %v, vars %v", after, api.last("CycleIssues").Vars)
	}
}
