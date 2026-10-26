package main

import (
	"encoding/json"
	"slices"
	"strings"
	"testing"
	"time"
)

// demoRun runs in demo mode with no key and an API URL nothing answers, so
// any request that left the process would fail.
func demoRun(t *testing.T, args ...string) result {
	t.Helper()
	return runWith(t, opts{env: []string{"LINEARCTL_DEMO=1"}}, args...)
}

func demoItems(t *testing.T, args ...string) []map[string]any {
	t.Helper()
	r := demoRun(t, append(args, "--json")...)
	if r.code != 0 {
		t.Fatalf("%v: exit %d, %q %q", args, r.code, r.out, r.err)
	}
	var d struct{ Items []map[string]any }
	must(t, json.Unmarshal([]byte(r.out), &d))
	return d.Items
}

func identifiers(items []map[string]any) []string {
	var out []string
	for _, it := range items {
		out = append(out, str(it, "identifier"))
	}
	return out
}

func TestDemoAnswersEveryReadCommandOffline(t *testing.T) {
	t.Parallel()
	args := map[string][]string{"issue show": {"ENG-142"}, "issue view": {"ENG-142"}, "comment list": {"ENG-142"},
		"project show": {"Checkout v2"}}
	skip := []string{"describe", "watch", "serve", "api query", "file download", "config show"}
	for _, c := range commands(t) {
		name := strings.Join(c.Path, " ")
		if c.Mutates || slices.Contains(skip, name) {
			continue
		}
		r := demoRun(t, append(append(slices.Clone(c.Path), args[name]...), "--json")...)
		var doc map[string]any
		if r.code != 0 || json.Unmarshal([]byte(r.out), &doc) != nil {
			t.Errorf("%s: exit %d, %q %q", name, r.code, r.out, r.err)
		}
	}
	if r := demoRun(t, "auth", "status", "--json"); !strings.Contains(r.out, `"source":"demo"`) || !strings.Contains(r.out, "Northwind") {
		t.Errorf("auth status: %q", r.out)
	}
}

func TestDemoFiltersAndPaging(t *testing.T) {
	t.Parallel()
	all := demoItems(t, "issue", "list", "--limit", "0")
	if len(all) != 17 {
		t.Fatalf("%d issues, want 17", len(all))
	}
	for i := 1; i < len(all); i++ {
		if str(all[i-1], "updatedAt") < str(all[i], "updatedAt") {
			t.Errorf("not newest first at %d", i)
		}
	}
	newest, _ := time.Parse(time.RFC3339, str(all[0], "updatedAt"))
	if since := time.Since(newest); since < 0 || since > 48*time.Hour {
		t.Errorf("times are not moved to now: newest %v", newest)
	}
	// Paging continues where it left off and loses nothing.
	var paged []string
	cursor := ""
	for range 10 {
		args := []string{"issue", "list", "--limit", "5", "--json"}
		if cursor != "" {
			args = append(args, "--cursor", cursor)
		}
		r := demoRun(t, args...)
		var d struct {
			Items      []map[string]any
			NextCursor *string
		}
		must(t, json.Unmarshal([]byte(r.out), &d))
		paged = append(paged, identifiers(d.Items)...)
		if d.NextCursor == nil {
			break
		}
		cursor = *d.NextCursor
	}
	if !slices.Equal(paged, identifiers(all)) {
		t.Errorf("paged %v\n  all %v", paged, identifiers(all))
	}
	every := func(name string, items []map[string]any, ok func(map[string]any) bool) {
		if len(items) == 0 {
			t.Errorf("%s: nothing matched", name)
		}
		for _, it := range items {
			if !ok(it) {
				t.Errorf("%s: %s does not belong", name, str(it, "identifier"))
			}
		}
	}
	has := func(it map[string]any, label string) bool {
		for _, l := range it["labels"].([]any) {
			if strings.EqualFold(str(l.(map[string]any), "name"), label) {
				return true
			}
		}
		return false
	}
	every("--label bug", demoItems(t, "issue", "list", "--label", "bug", "--limit", "0"), func(it map[string]any) bool { return has(it, "Bug") })
	every("--without-label bug", demoItems(t, "issue", "list", "--without-label", "bug", "--limit", "0"), func(it map[string]any) bool { return !has(it, "Bug") })
	every("--assignee me", demoItems(t, "issue", "list", "--assignee", "me", "--limit", "0"), func(it map[string]any) bool {
		return str(it, "assignee.email") == "maya@northwind.example"
	})
	every("--assignee none", demoItems(t, "issue", "list", "--assignee", "none", "--limit", "0"), func(it map[string]any) bool { return it["assignee"] == nil })
	every("--state-type started", demoItems(t, "issue", "list", "--state-type", "started", "--limit", "0"), func(it map[string]any) bool {
		return str(it, "state.type") == "started"
	})
	every("--cycle current", demoItems(t, "issue", "list", "--cycle", "current", "--limit", "0"), func(it map[string]any) bool { return it["cycle"] != nil })
	every("--project", demoItems(t, "issue", "list", "--project", "checkout v2", "--limit", "0"), func(it map[string]any) bool {
		return str(it, "project.name") == "Checkout v2"
	})
	every("--query coupon", demoItems(t, "issue", "list", "--query", "coupon"), func(it map[string]any) bool {
		return strings.Contains(strings.ToLower(str(it, "title")), "coupon")
	})
	if ids := identifiers(demoItems(t, "issue", "list", "--id", "eng-142", "--id", "ENG-131")); len(ids) != 2 {
		t.Errorf("--id: %v", ids)
	}
	prios := []float64{}
	for _, it := range demoItems(t, "issue", "list", "--sort", "priority", "--limit", "0") {
		p := it["priority"].(float64)
		if p == 0 {
			p = 5
		}
		prios = append(prios, p)
	}
	if !slices.IsSorted(prios) {
		t.Errorf("--sort priority: %v", prios)
	}
	if cur := demoItems(t, "cycle", "list"); len(cur) != 3 {
		t.Errorf("cycles: %v", cur)
	}
}

func TestDemoIssueDetail(t *testing.T) {
	t.Parallel()
	r := demoRun(t, "issue", "show", "ENG-142", "--json")
	var d struct{ Issue map[string]any }
	must(t, json.Unmarshal([]byte(r.out), &d))
	is := d.Issue
	comments := is["comments"].([]any)
	if r.code != 0 || len(comments) < 2 || str(is, "parent.identifier") != "ENG-120" || len(is["attachments"].([]any)) != 1 ||
		len(is["relations"].([]any)) != 1 || !strings.HasPrefix(str(is, "url"), "https://linear.app/northwind/issue/ENG-142/") {
		t.Fatalf("exit %d, %q", r.code, r.out)
	}
	replies := 0
	for _, c := range comments {
		if c.(map[string]any)["parent"] != nil {
			replies++
		}
	}
	if replies != 1 {
		t.Errorf("%d replies in the thread", replies)
	}
	r = demoRun(t, "issue", "show", "ENG-131", "--fields", "inverseRelations", "--json")
	if !strings.Contains(r.out, `"ENG-142"`) {
		t.Errorf("inverse relation: %q", r.out)
	}
	if r := demoRun(t, "issue", "show", "ENG-9999", "--json"); r.code != exitFailed || !strings.Contains(r.out, "not found: issue ENG-9999") {
		t.Errorf("missing: exit %d, %q", r.code, r.out)
	}
}

func TestDemoIsReadOnly(t *testing.T) {
	t.Parallel()
	r := demoRun(t, "issue", "create", "--team", "ENG", "--title", "x", "--label", "bug", "--state", "todo", "--assignee", "me", "--dry-run", "--json")
	in, _ := decode(t, r.out)["input"].(map[string]any)
	if r.code != 0 || in["teamId"] == nil || in["stateId"] == nil || in["assigneeId"] == nil || len(in["labelIds"].([]any)) != 1 {
		t.Errorf("dry run: exit %d, %q", r.code, r.out)
	}
	r = demoRun(t, "issue", "update", "ENG-142", "--state", "Done", "--dry-run", "--json")
	if d := decode(t, r.out); r.code != 0 || d["changed"] != true {
		t.Errorf("update dry run: exit %d, %q", r.code, r.out)
	}
	for _, args := range [][]string{
		{"issue", "update", "ENG-142", "--state", "Done", "--yes"},
		{"issue", "create", "--team", "ENG", "--title", "x", "--id", newIssueID, "--yes"},
		{"config", "set", "default-team", "ENG", "--yes"},
		{"file", "download", "https://uploads.linear.app/a/b", "--dry-run"},
	} {
		r := demoRun(t, append(args, "--json")...)
		if e, _ := decode(t, r.out)["error"].(string); r.code != exitFailed || !strings.Contains(e, "demo mode") {
			t.Errorf("%v: exit %d, %q", args, r.code, r.out)
		}
	}
}
