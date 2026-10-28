package acceptance

import (
	"strings"
	"testing"
)

// placeFake answers PlaceResolve with these projects and milestones, and
// IssueList with no issues.
func placeFake(t *testing.T, projects, milestones []any) *fake {
	return listFake(t, conn("")).on("PlaceResolve", map[string]any{
		"projects": nodes(projects...), "projectMilestones": nodes(milestones...)})
}

func project(id, name, slug string) map[string]any {
	return map[string]any{"id": id, "name": name, "slugId": slug, "url": "https://linear.app/acme/project/x-" + slug}
}

// --project takes what project list and project show hand out, and
// --milestone a milestone's id or name; both become ids before the issues
// are read, with one request.
func TestIssueListResolvesProjectAndMilestone(t *testing.T) {
	t.Parallel()
	q4 := []any{project("p-q4", "Q4 launch", "0527ca4c9128")}
	beta := []any{milestone("m-beta", "Beta", "p-q4")}
	cases := []struct {
		args       []string
		milestones []any
		project    string // PlaceResolve's $project, as JSON; "" when not asked
		milestone  string // and $milestone
		filter     string // IssueList's filter
	}{
		{[]string{"--project", "5F1E2A3B-4C5D-4E6F-8A9B-0C1D2E3F4A5B"}, nil,
			`{"id":{"eq":"5f1e2a3b-4c5d-4e6f-8a9b-0c1d2e3f4a5b"}}`, "", `{"project":{"id":{"eq":"p-q4"}}}`},
		{[]string{"--project", "0527ca4c9128"}, nil, `{"slugId":{"eq":"0527ca4c9128"}}`, "", `{"project":{"id":{"eq":"p-q4"}}}`},
		{[]string{"--project", "https://linear.app/acme/project/q4-launch-0527ca4c9128/overview"}, nil,
			`{"slugId":{"eq":"0527ca4c9128"}}`, "", `{"project":{"id":{"eq":"p-q4"}}}`},
		{[]string{"--project", "q4 LAUNCH"}, nil, `{"name":{"eqIgnoreCase":"q4 LAUNCH"}}`, "", `{"project":{"id":{"eq":"p-q4"}}}`},
		{[]string{"--milestone", "5F1E2A3B-4C5D-4E6F-8A9B-0C1D2E3F4A5B"}, beta,
			"", `{"id":{"eq":"5f1e2a3b-4c5d-4e6f-8a9b-0c1d2e3f4a5b"}}`, `{"projectMilestone":{"id":{"eq":"m-beta"}}}`},
		{[]string{"--milestone", "beta"}, beta, "", `{"name":{"eqIgnoreCase":"beta"}}`, `{"projectMilestone":{"id":{"eq":"m-beta"}}}`},
		{[]string{"--project", "Q4 launch", "--milestone", "beta"}, beta, `{"name":{"eqIgnoreCase":"Q4 launch"}}`,
			`{"name":{"eqIgnoreCase":"beta"},"project":{"name":{"eqIgnoreCase":"Q4 launch"}}}`,
			`{"project":{"id":{"eq":"p-q4"}},"projectMilestone":{"id":{"eq":"m-beta"}}}`},
		{[]string{"--project", "0527ca4c9128", "--milestone", "none"}, nil, `{"slugId":{"eq":"0527ca4c9128"}}`, "",
			`{"project":{"id":{"eq":"p-q4"}},"projectMilestone":{"null":true}}`},
	}
	for _, c := range cases {
		api := placeFake(t, q4, c.milestones)
		r := runWith(t, opts{api: api, key: testKey}, append([]string{"issue", "list", "--json"}, c.args...)...)
		if r.code != 0 {
			t.Errorf("%v: exit %d, %s", c.args, r.code, r.out)
			continue
		}
		if ops := strings.Join(api.ops(), ","); ops != "PlaceResolve,IssueList" {
			t.Errorf("%v: requests %s", c.args, ops)
		}
		v := api.last("PlaceResolve").Vars
		if v["withProject"] != (c.project != "") || (c.project != "" && jsonOf(v["project"]) != c.project) {
			t.Errorf("%v: project %v %s", c.args, v["withProject"], jsonOf(v["project"]))
		}
		if v["withMilestone"] != (c.milestone != "") || (c.milestone != "" && jsonOf(v["milestone"]) != c.milestone) {
			t.Errorf("%v: milestone %v %s", c.args, v["withMilestone"], jsonOf(v["milestone"]))
		}
		if got := jsonOf(api.last("IssueList").Vars["filter"]); got != c.filter {
			t.Errorf("%v: filter %s, want %s", c.args, got, c.filter)
		}
	}
}

// none is a filter of its own: no lookup.
func TestIssueListNoneNeedsNoLookup(t *testing.T) {
	t.Parallel()
	for args, want := range map[string]string{
		"--project none":                  `{"project":{"null":true}}`,
		"--milestone none":                `{"projectMilestone":{"null":true}}`,
		"--project none --milestone none": `{"project":{"null":true},"projectMilestone":{"null":true}}`,
	} {
		api := listFake(t, conn(""))
		r := runWith(t, opts{api: api, key: testKey}, append([]string{"issue", "list", "--json"}, strings.Fields(args)...)...)
		if ops := strings.Join(api.ops(), ","); r.code != 0 || ops != "IssueList" {
			t.Errorf("%s: exit %d, requests %s", args, r.code, ops)
		}
		if got := jsonOf(api.last("IssueList").Vars["filter"]); got != want {
			t.Errorf("%s: filter %s", args, got)
		}
	}
}

// A reference that finds nothing, or a name more than one project or
// milestone has, fails instead of listing no issues or a mix.
func TestIssueListUnresolvablePlaces(t *testing.T) {
	t.Parallel()
	two := []any{project("p-1", "Q4", "aaaaaaaaaaaa"), project("p-2", "q4", "bbbbbbbbbbbb")}
	betas := []any{milestone("m-1", "Beta", "p-1"), milestone("m-2", "Beta", "p-2")}
	cases := []struct {
		args       []string
		projects   []any
		milestones []any
		code, kind string
		name       string
		err        string
		candidates string
	}{
		{[]string{"--project", "Nope"}, nil, nil, "NOT_FOUND", "project", "Nope", "not found: project Nope (or this API key cannot see it)", ""},
		{[]string{"--project", "0527ca4c9128"}, nil, nil, "NOT_FOUND", "project", "0527ca4c9128", "not found: project 0527ca4c9128", ""},
		{[]string{"--project", "q4"}, two, nil, "AMBIGUOUS", "project", "q4",
			`more than one project is named "q4" (https://linear.app/acme/project/x-aaaaaaaaaaaa, https://linear.app/acme/project/x-bbbbbbbbbbbb); pass its URL or slugId`,
			`[{"id":"p-1","name":"Q4","slugId":"aaaaaaaaaaaa"},{"id":"p-2","name":"q4","slugId":"bbbbbbbbbbbb"}]`},
		{[]string{"--milestone", "Beta"}, nil, betas, "AMBIGUOUS", "milestone", "Beta",
			`2 milestones are named "Beta" (projects: name-p-1, name-p-2); pass --project or its id`,
			`[{"id":"m-1","name":"Beta","project":"name-p-1"},{"id":"m-2","name":"Beta","project":"name-p-2"}]`},
		{[]string{"--milestone", "GA"}, nil, nil, "NOT_FOUND", "milestone", "GA", `not found: milestone "GA" (or this API key cannot see it)`, ""},
		{[]string{"--project", "aaaaaaaaaaaa", "--milestone", "GA"}, two[:1], nil, "NOT_FOUND", "milestone", "GA",
			`not found: milestone "GA" in project "aaaaaaaaaaaa"`, ""},
		{[]string{"--project", "aaaaaaaaaaaa", "--milestone", "Beta"}, two[:1], betas, "AMBIGUOUS", "milestone", "Beta",
			`2 milestones are named "Beta" in project "aaaaaaaaaaaa" (projects: name-p-1, name-p-2); pass its id`, ""},
		// The project is checked first: its error is the one reported.
		{[]string{"--project", "q4", "--milestone", "Beta"}, two, betas, "AMBIGUOUS", "project", "q4", "more than one project", ""},
	}
	for _, c := range cases {
		api := placeFake(t, c.projects, c.milestones)
		r := runWith(t, opts{api: api, key: testKey}, append([]string{"issue", "list", "--json"}, c.args...)...)
		d := decode(t, r.out)
		e, _ := d["error"].(string)
		if r.code != exitFailed || d["code"] != c.code || d["kind"] != c.kind || d["name"] != c.name || !strings.Contains(e, c.err) {
			t.Errorf("%v: exit %d, %s", c.args, r.code, r.out)
		}
		if c.candidates != "" && jsonOf(d["candidates"]) != c.candidates {
			t.Errorf("%v: candidates %s", c.args, jsonOf(d["candidates"]))
		}
		if ops := strings.Join(api.ops(), ","); ops != "PlaceResolve" {
			t.Errorf("%v: requests %s", c.args, ops)
		}
	}
}

// The lookup failing ends the list before any issue is read.
func TestIssueListPlaceLookupFails(t *testing.T) {
	t.Parallel()
	api := listFake(t, conn("")).onFunc("PlaceResolve", linearErr("boom", "INTERNAL_ERROR"))
	r := runWith(t, opts{api: api, key: testKey}, "issue", "list", "--json", "--project", "Q4")
	if d := decode(t, r.out); r.code != exitFailed || d["code"] != "LINEAR_ERROR" {
		t.Errorf("exit %d, %s", r.code, r.out)
	}
	if ops := strings.Join(api.ops(), ","); ops != "PlaceResolve" {
		t.Errorf("requests %s", ops)
	}
}

func TestIssueListRefusesBadPlaces(t *testing.T) {
	t.Parallel()
	for _, args := range [][]string{
		{"--project", "https://linear.app/acme/issue/ENG-1"},
		{"--project", "https://linear.app/acme/project/no-id-here"},
		{"--project", "a\x07"},
		{"--milestone", "a\x07"},
		{"--project", "none", "--milestone", "Beta"},
	} {
		api := newFake(t)
		r := runWith(t, opts{api: api, key: testKey}, append([]string{"issue", "list", "--json"}, args...)...)
		if r.code != exitUsage || decode(t, r.out)["usage"] != true || len(api.calls()) != 0 {
			t.Errorf("%v: exit %d, %q, requests %v", args, r.code, r.out, api.ops())
		}
	}
}

// watch reads the same flags: it resolves them once, before its snapshot,
// and filters by the ids from then on.
func TestWatchResolvesProject(t *testing.T) {
	t.Parallel()
	api := newWatchFake(t)
	api.on("PlaceResolve", map[string]any{"projects": nodes(project("p-q4", "Q4 launch", "0527ca4c9128")),
		"projectMilestones": nodes(milestone("m-beta", "Beta", "p-q4"))})
	s := startWatch(t, api.fake, "--project", "https://linear.app/acme/project/q4-launch-0527ca4c9128",
		"--milestone", "beta", "--mode", "poll", "--poll-interval", "10")
	s.next("ready")
	if code := s.stop(); code != 130 {
		t.Errorf("exit %d", code)
	}
	if ops := api.ops(); len(ops) < 2 || ops[0] != "PlaceResolve" || ops[1] != "WatchIDs" {
		t.Errorf("requests %v", ops)
	}
	v := api.last("PlaceResolve").Vars
	if jsonOf(v["project"]) != `{"slugId":{"eq":"0527ca4c9128"}}` ||
		jsonOf(v["milestone"]) != `{"name":{"eqIgnoreCase":"beta"},"project":{"slugId":{"eq":"0527ca4c9128"}}}` {
		t.Errorf("PlaceResolve vars %v", v)
	}
	want := `{"project":{"id":{"eq":"p-q4"}},"projectMilestone":{"id":{"eq":"m-beta"}}}`
	if f := jsonOf(api.last("WatchIDs").Vars["filter"]); f != want {
		t.Errorf("snapshot filter %s", f)
	}
}

func TestWatchUnknownProjectFails(t *testing.T) {
	t.Parallel()
	api := newFake(t).on("PlaceResolve", map[string]any{"projects": nodes()})
	r := runWith(t, opts{api: api, key: testKey}, "watch", "--json", "--project", "Nope")
	d := decode(t, r.out)
	if r.code != exitFailed || d["code"] != "NOT_FOUND" || d["kind"] != "project" || d["name"] != "Nope" {
		t.Errorf("exit %d, %s", r.code, r.out)
	}
	if ops := strings.Join(api.ops(), ","); ops != "PlaceResolve" {
		t.Errorf("requests %s", ops)
	}
}
