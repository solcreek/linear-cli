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
