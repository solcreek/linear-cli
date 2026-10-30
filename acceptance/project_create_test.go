package acceptance

import (
	"slices"
	"testing"
)

const (
	newProjectID = "7d1e2a3b-4c5d-4e6f-8a9b-0c1d2e3f4a5b"
	engTeamID    = "3a1e2a3b-4c5d-4e6f-8a9b-0c1d2e3f4a5b"
)

// projectCreateResolve is ProjectCreateResolve's answer: the teams ENG and
// DES, maya as the one user an email finds, two statuses, and existing as
// the project the chosen id names, if any.
func projectCreateResolve(existing ...any) map[string]any {
	return map[string]any{
		"teams":    conn("", map[string]any{"id": "t-des", "key": "DES"}, map[string]any{"id": engTeamID, "key": "ENG"}),
		"existing": conn("", existing...),
		"users":    conn("", map[string]any{"id": "u-maya", "name": "Maya", "email": "maya@acme.test"}),
		"viewer":   map[string]any{"id": "u-me"},
		"projectStatuses": conn("", map[string]any{"id": "s-plan", "name": "Planned", "type": "planned"},
			map[string]any{"id": "s-go", "name": "In Progress", "type": "started"}),
	}
}

func TestProjectCreate(t *testing.T) {
	t.Parallel()
	resolve := func(existing ...any) *fake {
		return newFake(t).on("ProjectCreateResolve", projectCreateResolve(existing...)).
			on("ProjectCreate", map[string]any{"projectCreate": map[string]any{"success": true, "project": projectNode("q4")}})
	}
	api := resolve()
	r := runWith(t, opts{api: api, key: testKey, stdin: "# Overview"}, "project", "create", "--team", "eng", "--team", "DES",
		"--name", "Q4 launch", "--content-file", "-", "--lead", "maya@acme.test", "--status", "planned", "--priority", "high",
		"--target-date", "2026-12-15", "--dry-run", "--json")
	d := decode(t, r.out)
	in, _ := d["input"].(map[string]any)
	if r.code != 0 || jsonOf(in["teamIds"]) != `["`+engTeamID+`","t-des"]` || in["leadId"] != "u-maya" || in["statusId"] != "s-plan" ||
		in["priority"] != 2.0 || in["targetDate"] != "2026-12-15" || in["content"] != "# Overview" || in["id"] != d["id"] {
		t.Fatalf("dry run: exit %d, %q", r.code, r.out)
	}
	v := api.last("ProjectCreateResolve").Vars
	if jsonOf(v["teams"]) != `{"or":[{"key":{"eqIgnoreCase":"ENG"}},{"key":{"eqIgnoreCase":"DES"}}]}` || v["withStatus"] != true || v["me"] != false {
		t.Errorf("resolve vars %v", v)
	}
	if slices.Contains(api.ops(), "ProjectCreate") {
		t.Error("a dry run created")
	}

	// --json-input is Linear's input as is; flags win over its fields.
	r = runWith(t, opts{api: api, key: testKey, stdin: `{"name": "From JSON", "icon": "Rocket", "teamIds": ["` + engTeamID + `"], "priority": 4}`},
		"project", "create", "--json-input", "-", "--priority", "urgent", "--lead", "me", "--id", newProjectID, "--yes", "--json")
	in, _ = api.last("ProjectCreate").Vars["input"].(map[string]any)
	if r.code != 0 || in["id"] != newProjectID || in["name"] != "From JSON" || in["icon"] != "Rocket" || in["priority"] != 1.0 ||
		in["leadId"] != "u-me" || jsonOf(in["teamIds"]) != `["`+engTeamID+`"]` || decode(t, r.out)["project"] == nil {
		t.Errorf("yes: exit %d, %q, input %v", r.code, r.out, in)
	}
	if v := api.last("ProjectCreateResolve").Vars; jsonOf(v["teams"]) != `{"or":[{"id":{"eq":"`+engTeamID+`"}}]}` {
		t.Errorf("teamIds resolved as %v", v["teams"])
	}

	// A rerun with the same id finds the project and creates nothing.
	api = resolve(projectNode("q4"))
	r = runWith(t, opts{api: api, key: testKey}, "project", "create", "--team", "ENG", "--name", "Q4", "--id", newProjectID, "--yes", "--json")
	if r.code != 0 || decode(t, r.out)["changed"] != false || slices.Contains(api.ops(), "ProjectCreate") {
		t.Errorf("rerun: exit %d, %q", r.code, r.out)
	}

	for _, args := range [][]string{{"--team", "OPS"}, {"--status", "Shipped"}, {"--lead", "nobody@acme.test"}} {
		none := projectCreateResolve()
		none["users"] = conn("")
		api := newFake(t).on("ProjectCreateResolve", none)
		r := runWith(t, opts{api: api, key: testKey}, append([]string{"project", "create", "--team", "ENG", "--name", "Q4", "--dry-run", "--json"}, args...)...)
		if d := decode(t, r.out); r.code != exitFailed || d["code"] != "NOT_FOUND" {
			t.Errorf("%v: exit %d, %q", args, r.code, r.out)
		}
	}

	api = resolve()
	for _, args := range [][]string{
		{"--team", "ENG", "--name", "Q4", "--yes"}, // no --id
		{"--team", "ENG", "--dry-run"},             // no name
		{"--name", "Q4", "--dry-run"},              // no team
		{"--team", "ENG", "--name", "Q4", "--target-date", "soon", "--dry-run"},
		{"--team", "ENG", "--name", "Q4", "--lead", "maya", "--dry-run"},
		{"--team", "ENG", "--name", "Q4", "--priority", "9", "--dry-run"},
		{"--team", "ENG", "--name", "Q4", "--id", "nope", "--yes"},
	} {
		if r := runWith(t, opts{api: api, key: testKey}, append([]string{"project", "create"}, args...)...); r.code != exitUsage {
			t.Errorf("%v: exit %d, %q", args, r.code, r.err)
		}
	}
	if len(api.calls()) != 0 {
		t.Error("a refused create reached Linear")
	}
}
