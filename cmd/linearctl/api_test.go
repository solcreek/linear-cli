package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestAPIQuery(t *testing.T) {
	t.Parallel()
	api := newFake(t).onFunc("Me", func(vars map[string]any) (int, any, map[string]string) {
		return 200, map[string]any{"data": map[string]any{"viewer": map[string]any{"name": "Ada"}}}, map[string]string{"X-Complexity": "2"}
	})
	req := `{"query":"query Me($n: Int) { viewer { name } }","variables":{"n":1},"operationName":"Me"}`
	r := runWith(t, opts{api: api, key: testKey, stdin: req}, "api", "query", "--json-input", "-", "--signed-urls", "60")
	d := decode(t, r.out)
	if r.code != 0 || d["complexity"] != 2.0 || d["data"].(map[string]any)["viewer"].(map[string]any)["name"] != "Ada" {
		t.Fatalf("exit %d, %q %q", r.code, r.out, r.err)
	}
	c := api.last("Me")
	if c.Vars["n"] != 1.0 || c.Header.Get("public-file-urls-expire-in") != "60" {
		t.Errorf("sent %v, %v", c.Vars, c.Header)
	}
}

func TestAPIQueryPartialAndFailed(t *testing.T) {
	t.Parallel()
	api := newFake(t).onFunc("Two", func(map[string]any) (int, any, map[string]string) {
		return 200, map[string]any{"data": map[string]any{"viewer": map[string]any{"name": "Ada"}, "issue": nil},
			"errors": []any{map[string]any{"message": "Entity not found: Issue", "path": []any{"issue"}}}}, nil
	}).onFunc("Bad", linearErr("Cannot query field \"nope\"", "GRAPHQL_VALIDATION_FAILED"))
	r := runWith(t, opts{api: api, key: testKey, stdin: `{"query":"query Two { viewer { name } issue(id: \"X-1\") { id } }"}`}, "api", "query", "--json-input", "-")
	p, _ := decode(t, r.out)["problems"].([]any)
	if r.code != exitPartial || len(p) != 1 || p[0].(map[string]any)["input"] != "issue" {
		t.Errorf("partial: exit %d, %q", r.code, r.out)
	}
	r = runWith(t, opts{api: api, key: testKey, stdin: `{"query":"query Bad { nope }"}`}, "api", "query", "--json-input", "-", "--json")
	if e, _ := decode(t, r.out)["error"].(string); r.code != exitFailed || !strings.Contains(e, "nope") {
		t.Errorf("failed: exit %d, %q", r.code, r.out)
	}
}

func TestAPIQueryRefusesChangesAndBadInput(t *testing.T) {
	t.Parallel()
	dir := t.TempDir()
	must(t, os.WriteFile(filepath.Join(dir, "m.json"), []byte(`{"query":"mutation M { issueArchive(id: \"x\") { success } }"}`), 0o644))
	api := newFake(t)
	cases := []struct {
		args  []string
		stdin string
	}{
		{[]string{"--json-input", "m.json"}, ""},
		{[]string{"--json-input", "-"}, `{"query":"query A { viewer { id } } subscription S { x }"}`},
		{[]string{"--json-input", "-"}, `{"query":"query {"}`},
		{[]string{"--json-input", "-"}, `{"query":""}`},
		{[]string{"--json-input", "-"}, `[]`},
		{nil, ""},
	}
	for _, c := range cases {
		r := runWith(t, opts{dir: dir, api: api, key: testKey, stdin: c.stdin}, append([]string{"api", "query", "--json"}, c.args...)...)
		if r.code != exitUsage {
			t.Errorf("%v %q: exit %d, %q", c.args, c.stdin, r.code, r.out)
		}
	}
	if len(api.calls()) != 0 {
		t.Errorf("sent a refused request: %v", api.ops())
	}
}
