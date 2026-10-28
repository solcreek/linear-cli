package acceptance

import (
	"encoding/json"
	"testing"
)

// Every failure under --json carries "code", so a caller branches on it
// rather than on the words of "error"; describe lists the codes.
func TestFailureCodes(t *testing.T) {
	t.Parallel()
	limited := newFake(t).onFunc("IssueShow", func(map[string]any) (int, any, map[string]string) {
		return 400, map[string]any{"errors": []any{map[string]any{"message": "x", "extensions": map[string]any{"code": "RATELIMITED"}}}}, nil
	})
	cases := []struct {
		name string
		o    opts
		args []string
		code string
		exit int
	}{
		{"usage", opts{key: testKey}, []string{"issue", "show", "nope"}, "USAGE", exitUsage},
		{"not found", opts{api: newFake(t).onFunc("IssueShow", linearErr("Entity not found: Issue", "INPUT_ERROR")), key: testKey},
			[]string{"issue", "show", "ENG-9"}, "NOT_FOUND", exitFailed},
		{"ambiguous", opts{api: newFake(t).on("ProjectFind", projectFind(projectNode("a"), projectNode("b"))), key: testKey},
			[]string{"project", "show", "Twin"}, "AMBIGUOUS", exitFailed},
		{"linear error", opts{api: newFake(t).onFunc("IssueShow", linearErr("Title is too long", "INPUT_ERROR")), key: testKey},
			[]string{"issue", "show", "ENG-9"}, "LINEAR_ERROR", exitFailed},
		{"network", opts{key: testKey}, []string{"issue", "show", "ENG-9"}, "NETWORK", exitFailed},
		{"auth", opts{api: newFake(t)}, []string{"issue", "show", "ENG-9"}, "AUTH", exitAuth},
		{"rate limited", opts{api: limited, key: testKey}, []string{"issue", "show", "ENG-9"}, "RATE_LIMITED", exitLimited},
		{"conflict", opts{api: updateFake(t), key: testKey},
			[]string{"issue", "update", "ENG-5", "--title", "x", "--if-updated-at", "2020-01-01T00:00:00Z", "--yes"}, "CONFLICT", exitConflict},
	}
	for _, c := range cases {
		r := runWith(t, c.o, append(c.args, "--json")...)
		if d := decode(t, r.out); r.code != c.exit || d["code"] != c.code {
			t.Errorf("%s: exit %d, %q", c.name, r.code, r.out)
		}
	}
	var d struct {
		ExitCodes  map[string]int
		ErrorCodes []struct {
			Code    string
			Exit    int
			Meaning string
		}
	}
	must(t, json.Unmarshal([]byte(run(t, "describe").out), &d))
	exits := map[int]bool{}
	for _, e := range d.ExitCodes {
		exits[e] = true
	}
	seen := map[string]bool{}
	for _, e := range d.ErrorCodes {
		if !exits[e.Exit] || e.Meaning == "" || seen[e.Code] {
			t.Errorf("errorCodes entry %+v", e)
		}
		seen[e.Code] = true
	}
	for _, c := range cases {
		if !seen[c.code] {
			t.Errorf("describe does not list %s", c.code)
		}
	}
}
