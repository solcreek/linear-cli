package acceptance

import (
	"strings"
	"testing"
)

func milestone(id, name, project string) map[string]any {
	return map[string]any{"id": id, "name": name, "project": map[string]any{"id": project, "name": "name-" + project}}
}

func TestIssueCreateMilestone(t *testing.T) {
	t.Parallel()
	two := conn("", milestone("m-q4", "Beta", "p-q4"), milestone("m-other", "Beta", "p-other"))
	cases := []struct {
		args       []string
		milestones map[string]any
		want       string // input fields, as JSON subset; "" for an error
		err        string
	}{
		{[]string{"--project", "Q4", "--milestone", "beta"}, two, `"projectId":"p-q4","projectMilestoneId":"m-q4"`, ""},
		{[]string{"--milestone", "Beta"}, two, "", `2 milestones are named "Beta" (projects: name-p-q4, name-p-other); pass --project`},
		{[]string{"--milestone", "Solo"}, conn("", milestone("m-solo", "Solo", "p-solo")), `"projectId":"p-solo","projectMilestoneId":"m-solo"`, ""},
		{[]string{"--project", "Q4", "--milestone", "Beta"}, conn("", milestone("m-other", "Beta", "p-other")), "", `no milestone named "Beta" in the issue's project`},
	}
	for _, c := range cases {
		api := createFake(t, resolveAnswer(map[string]any{"milestones": c.milestones}))
		r := runWith(t, opts{api: api, key: testKey}, append([]string{"issue", "create", "--team", "ENG", "--title", "x", "--dry-run", "--json"}, c.args...)...)
		d := decode(t, r.out)
		if c.err != "" {
			if e, _ := d["error"].(string); r.code != exitFailed || !strings.Contains(e, c.err) {
				t.Errorf("%v: exit %d, %q", c.args, r.code, r.out)
			}
			continue
		}
		if r.code != 0 || !strings.Contains(jsonOf(d["input"]), c.want) {
			t.Errorf("%v: exit %d, input %s", c.args, r.code, jsonOf(d["input"]))
		}
		if v := api.last("CreateResolve").Vars; v["withMilestone"] != true || v["milestone"] != c.args[len(c.args)-1] {
			t.Errorf("%v: vars %v", c.args, v)
		}
	}
}

func TestIssueUpdateMilestone(t *testing.T) {
	t.Parallel()
	answer := func() map[string]any {
		is := currentIssue()
		is["project"] = map[string]any{"id": "p-q4", "name": "Q4"}
		is["projectMilestone"] = map[string]any{"id": "m-q4"}
		return resolveAnswer(map[string]any{"issue": is,
			"milestones": conn("", milestone("m-q4", "Beta", "p-q4"), milestone("m-other", "Beta", "p-other"), milestone("m-ga", "GA", "p-q4"))})
	}
	for _, c := range []struct {
		args []string
		want string
	}{
		{[]string{"--milestone", "Beta"}, ""}, // already there; the issue's project decides
		{[]string{"--milestone", "GA"}, `{"projectMilestoneId":"m-ga"}`},
		{[]string{"--milestone", "none"}, `{"projectMilestoneId":null}`},
	} {
		api := newFake(t).on("UpdateResolve", answer()).
			on("IssueUpdate", map[string]any{"issueUpdate": map[string]any{"issue": issueNode("ENG-5", "x")}})
		r := runWith(t, opts{api: api, key: testKey}, append([]string{"issue", "update", "ENG-5", "--dry-run", "--json"}, c.args...)...)
		d := decode(t, r.out)
		if r.code != 0 || d["changed"] != (c.want != "") || (c.want != "" && jsonOf(d["input"]) != c.want) {
			t.Errorf("%v: exit %d, %q", c.args, r.code, r.out)
		}
		if v := api.last("UpdateResolve").Vars; v["withMilestone"] != (c.args[1] != "none") {
			t.Errorf("%v: withMilestone %v", c.args, v["withMilestone"])
		}
	}
}

func TestMilestoneFilterAndList(t *testing.T) {
	t.Parallel()
	for arg, want := range map[string]string{"Beta": `{"projectMilestone":{"name":{"eqIgnoreCase":"Beta"}}}`, "none": `{"projectMilestone":{"null":true}}`} {
		api := listFake(t, conn(""))
		runWith(t, opts{api: api, key: testKey}, "issue", "list", "--milestone", arg)
		if got := jsonOf(api.last("IssueList").Vars["filter"]); got != want {
			t.Errorf("--milestone %s: %s", arg, got)
		}
	}
	api := newFake(t).on("Milestones", map[string]any{"projectMilestones": conn("", map[string]any{"id": "m1", "name": "Beta",
		"targetDate": "2026-11-30", "progress": 0.4, "project": map[string]any{"id": "p1", "name": "Q4"}})})
	r := runWith(t, opts{api: api, key: testKey}, "milestone", "list", "--project", "Q4")
	if r.code != 0 || r.out != "Q4\tBeta\t2026-11-30\n" || jsonOf(api.last("Milestones").Vars["filter"]) != `{"project":{"name":{"eqIgnoreCase":"Q4"}}}` {
		t.Errorf("exit %d, %q, %v", r.code, r.out, api.last("Milestones").Vars)
	}
}
