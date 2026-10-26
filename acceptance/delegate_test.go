package acceptance

import (
	"strings"
	"testing"
)

func TestIssueDelegate(t *testing.T) {
	t.Parallel()
	agent := conn("", map[string]any{"id": "u-bot", "name": "Codex"})
	api := createFake(t, resolveAnswer(map[string]any{"delegates": agent}))
	r := runWith(t, opts{api: api, key: testKey}, "issue", "create", "--team", "ENG", "--title", "x", "--delegate", "codex", "--dry-run", "--json")
	if d := decode(t, r.out); r.code != 0 || d["input"].(map[string]any)["delegateId"] != "u-bot" {
		t.Fatalf("create: exit %d, %q", r.code, r.out)
	}
	if v := api.last("CreateResolve").Vars; v["delegate"] != "codex" || v["withDelegate"] != true {
		t.Errorf("vars %v", v)
	}
	api = createFake(t, resolveAnswer(map[string]any{"delegates": conn("")}))
	r = runWith(t, opts{api: api, key: testKey}, "issue", "create", "--team", "ENG", "--title", "x", "--delegate", "Ada", "--dry-run", "--json")
	if e, _ := decode(t, r.out)["error"].(string); r.code != exitFailed || !strings.Contains(e, "user list --app") {
		t.Errorf("not an agent: exit %d, %q", r.code, r.out)
	}
	is := currentIssue()
	is["delegate"] = map[string]any{"id": "u-bot"}
	api = newFake(t).on("UpdateResolve", resolveAnswer(map[string]any{"issue": is, "delegates": agent}))
	for arg, want := range map[string]string{"Codex": "", "none": `{"delegateId":null}`} {
		r := runWith(t, opts{api: api, key: testKey}, "issue", "update", "ENG-5", "--delegate", arg, "--dry-run", "--json")
		if d := decode(t, r.out); r.code != 0 || d["changed"] != (want != "") || (want != "" && jsonOf(d["input"]) != want) {
			t.Errorf("update --delegate %s: exit %d, %q", arg, r.code, r.out)
		}
	}
}

func TestDelegateFilters(t *testing.T) {
	t.Parallel()
	for arg, want := range map[string]string{
		"none":  `{"delegate":{"null":true}}`,
		"any":   `{"delegate":{"null":false}}`,
		"Codex": `{"delegate":{"or":[{"name":{"eqIgnoreCase":"Codex"}},{"displayName":{"eqIgnoreCase":"Codex"}},{"email":{"eqIgnoreCase":"Codex"}}]}}`,
	} {
		api := listFake(t, conn(""))
		runWith(t, opts{api: api, key: testKey}, "issue", "list", "--delegate", arg)
		if got := jsonOf(api.last("IssueList").Vars["filter"]); got != want {
			t.Errorf("--delegate %s: %s", arg, got)
		}
	}
	api := newFake(t).on("Users", map[string]any{"users": conn("")})
	runWith(t, opts{api: api, key: testKey}, "user", "list", "--app", "--active")
	if got := jsonOf(api.last("Users").Vars["filter"]); got != `{"active":{"eq":true},"app":{"eq":true}}` {
		t.Errorf("user list --app: %s", got)
	}
}
