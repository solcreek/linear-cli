package acceptance

import (
	"slices"
	"strings"
	"testing"
)

func projectNode(name string) map[string]any {
	return map[string]any{"id": "p-" + name, "name": name, "slugId": "0527ca4c9128", "description": "Ship it",
		"content": "# Overview\nold", "url": "https://linear.app/acme/project/" + name + "-0527ca4c9128",
		"status": map[string]any{"name": "In Progress"}, "issues": nodes(issueNode("ENG-1", "One"))}
}

func TestProjectShowRefs(t *testing.T) {
	t.Parallel()
	api := newFake(t).on("ProjectFind", map[string]any{"projects": conn("", projectNode("q4"))})
	for ref, want := range map[string]string{
		"https://linear.app/acme/project/q4-launch-0527ca4c9128": `{"slugId":{"eq":"0527ca4c9128"}}`,
		"0527ca4c9128":                         `{"slugId":{"eq":"0527ca4c9128"}}`,
		"5F1E2A3B-4C5D-4E6F-8A9B-0C1D2E3F4A5B": `{"id":{"eq":"5f1e2a3b-4c5d-4e6f-8a9b-0c1d2e3f4a5b"}}`,
		"Q4 launch":                            `{"name":{"eqIgnoreCase":"Q4 launch"}}`,
	} {
		r := runWith(t, opts{api: api, key: testKey}, "project", "show", ref, "--json")
		if got := jsonOf(api.last("ProjectFind").Vars["filter"]); r.code != 0 || got != want {
			t.Errorf("%s: exit %d, filter %s", ref, r.code, got)
		}
	}
	v := api.last("ProjectFind").Vars
	if v["issues"] != 50.0 || v["withIssues"] != true {
		t.Errorf("vars %v", v)
	}
	n := len(api.calls())
	for _, bad := range []string{"https://linear.app/acme/issue/ENG-1", "https://linear.app/acme/project/no-id", "https://evil.example/acme/project/x-0527ca4c9128"} {
		if r := runWith(t, opts{api: api, key: testKey}, "project", "show", bad); r.code != exitUsage {
			t.Errorf("%s: exit %d", bad, r.code)
		}
	}
	if r := runWith(t, opts{api: api, key: testKey}, "project", "show", "q4", "--issues", "251"); r.code != exitUsage {
		t.Errorf("--issues 251: exit %d", r.code)
	}
	if len(api.calls()) != n {
		t.Error("a refused PROJECT reached Linear")
	}
	r := runWith(t, opts{api: api, key: testKey}, "project", "show", "q4", "--issues", "0")
	if r.code != 0 || !strings.Contains(r.out, "q4  (In Progress)") || !strings.Contains(r.out, "# Overview\nold") {
		t.Errorf("text: %q", r.out)
	}
}

func TestProjectShowNotFoundAndAmbiguous(t *testing.T) {
	t.Parallel()
	api := newFake(t).on("ProjectFind", map[string]any{"projects": conn("")})
	r := runWith(t, opts{api: api, key: testKey}, "project", "show", "Nope", "--json")
	if e, _ := decode(t, r.out)["error"].(string); r.code != exitFailed || !strings.Contains(e, "not found: project Nope") {
		t.Errorf("none: exit %d, %q", r.code, r.out)
	}
	api = newFake(t).on("ProjectFind", map[string]any{"projects": conn("", projectNode("a"), projectNode("b"))})
	r = runWith(t, opts{api: api, key: testKey}, "project", "show", "Twin", "--json")
	if e, _ := decode(t, r.out)["error"].(string); r.code != exitFailed || !strings.Contains(e, "more than one project is named") {
		t.Errorf("two: exit %d, %q", r.code, r.out)
	}
}

func TestProjectUpdate(t *testing.T) {
	t.Parallel()
	api := newFake(t).on("ProjectFind", map[string]any{"projects": conn("", projectNode("q4"))}).
		on("ProjectUpdate", map[string]any{"projectUpdate": map[string]any{"success": true}})
	r := runWith(t, opts{api: api, key: testKey, stdin: "# Overview\nnew"}, "project", "update", "q4",
		"--content-file", "-", "--description", "Ship it", "--dry-run", "--json")
	if d := decode(t, r.out); r.code != 0 || jsonOf(d["input"]) != `{"content":"# Overview\nnew"}` || slices.Contains(api.ops(), "ProjectUpdate") {
		t.Errorf("dry run: exit %d, %q", r.code, r.out)
	}
	if v := api.last("ProjectFind").Vars; v["withIssues"] != false {
		t.Errorf("update fetched issues: %v", v)
	}
	r = runWith(t, opts{api: api, key: testKey}, "project", "update", "q4", "--name", "Q4 launch", "--yes", "--json")
	if c := api.last("ProjectUpdate"); r.code != 0 || c.Vars["id"] != "p-q4" || jsonOf(c.Vars["input"]) != `{"name":"Q4 launch"}` {
		t.Errorf("yes: exit %d, %q", r.code, r.out)
	}
	n := len(api.calls())
	r = runWith(t, opts{api: api, key: testKey}, "project", "update", "q4", "--description", "Ship it", "--yes", "--json")
	if d := decode(t, r.out); r.code != 0 || d["changed"] != false || len(api.calls()) != n+1 {
		t.Errorf("same: exit %d, %q", r.code, r.out)
	}
	for _, args := range [][]string{{}, {"--name", ""}, {"--content", "a", "--content-file", "-"}} {
		if r := runWith(t, opts{api: api, key: testKey}, append([]string{"project", "update", "q4", "--dry-run"}, args...)...); r.code != exitUsage {
			t.Errorf("%v: exit %d", args, r.code)
		}
	}
}

func TestProjectShowReadsEveryDocument(t *testing.T) {
	t.Parallel()
	doc := func(n string) map[string]any {
		return map[string]any{"id": "d-" + n, "title": n, "url": "https://linear.app/acme/document/" + n}
	}
	p := projectNode("q4")
	p["health"], p["documents"] = "atRisk", conn("d1", doc("a"))
	p["teams"] = nodes(map[string]any{"id": "t-eng", "key": "ENG", "name": "Engineering"})
	var after []any
	api := newFake(t).on("ProjectFind", map[string]any{"projects": conn("", p)}).
		onFunc("ProjectDocuments", func(v map[string]any) (int, any, map[string]string) {
			after = append(after, v["after"])
			return 200, map[string]any{"data": map[string]any{"project": map[string]any{"documents": conn("", doc("b"))}}}, nil
		})
	r := runWith(t, opts{api: api, key: testKey}, "project", "show", "q4", "--fields", "health,teams,documents", "--json")
	got, _ := decode(t, r.out)["project"].(map[string]any)
	if r.code != 0 || got["health"] != "atRisk" || jsonOf(got["teams"]) != `[{"id":"t-eng","key":"ENG","name":"Engineering"}]` ||
		jsonOf(got["documents"]) != `[{"id":"d-a","title":"a","url":"https://linear.app/acme/document/a"},{"id":"d-b","title":"b","url":"https://linear.app/acme/document/b"}]` {
		t.Fatalf("exit %d, %q", r.code, r.out)
	}
	if jsonOf(after) != `["d1"]` || api.last("ProjectDocuments").Vars["id"] != "p-q4" {
		t.Errorf("follow-ups %v", after)
	}
	q := api.last("ProjectFind").Query
	for _, want := range []string{"health", "createdAt", "updatedAt", "teams(first: 20) { nodes { id key name } }"} {
		if !strings.Contains(q, want) {
			t.Errorf("ProjectFind lacks %q", want)
		}
	}
}

func TestProjectShowAllIssues(t *testing.T) {
	t.Parallel()
	p := projectNode("q4")
	p["issues"] = conn("i1", issueNode("ENG-1", "One"))
	var after []any
	api := newFake(t).on("ProjectFind", map[string]any{"projects": conn("", p)}).
		onFunc("ProjectIssues", func(v map[string]any) (int, any, map[string]string) {
			after = append(after, v["after"])
			page := conn("i2", issueNode("ENG-2", "Two"))
			if len(after)%2 == 0 {
				page = conn("", issueNode("ENG-3", "Three"))
			}
			return 200, map[string]any{"data": map[string]any{"project": map[string]any{"issues": page}}}, nil
		})
	r := runWith(t, opts{api: api, key: testKey}, "project", "show", "q4", "--all-issues", "--fields", "issues", "--json")
	got, _ := decode(t, r.out)["project"].(map[string]any)
	is, _ := got["issues"].([]any)
	if r.code != 0 || len(is) != 3 || r.err != "" {
		t.Fatalf("exit %d, %q, stderr %q", r.code, r.out, r.err)
	}
	for i, want := range []string{"ENG-1", "ENG-2", "ENG-3"} {
		if id := is[i].(map[string]any)["identifier"]; id != want {
			t.Errorf("issue %d is %v, want %s", i, id, want)
		}
	}
	if v := api.last("ProjectFind").Vars; v["issues"] != 250.0 || v["withIssues"] != true {
		t.Errorf("first page vars %v", v)
	}
	if jsonOf(after) != `["i1","i2"]` || api.last("ProjectIssues").Vars["id"] != "p-q4" {
		t.Errorf("follow-ups after %v, vars %v", after, api.last("ProjectIssues").Vars)
	}
	r = runWith(t, opts{api: api, key: testKey}, "project", "show", "q4", "--all-issues", "--issues", "999")
	if r.code != 0 || !strings.Contains(r.out, "\n3 issues\n") {
		t.Errorf("--all-issues over --issues 999: exit %d, %q", r.code, r.out)
	}
	if v := api.last("ProjectFind").Vars; v["issues"] != 250.0 {
		t.Errorf("--issues 999 reached Linear: %v", v)
	}
}
