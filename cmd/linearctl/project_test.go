package main

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
