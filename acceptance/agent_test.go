package acceptance

import (
	"strings"
	"testing"
)

// Linear's agent guide: when work starts, move the issue to the team's first
// started state (lowest position), and an agent sets itself as delegate.

func typedTeam(states ...any) map[string]any {
	return map[string]any{"teams": conn("", map[string]any{"id": "team-eng", "key": "ENG", "states": nodes(), "typed": nodes(states...)})}
}

func TestStateTypePicksTheFirstStateOfThatType(t *testing.T) {
	t.Parallel()
	api := createFake(t, resolveAnswer(typedTeam(
		map[string]any{"id": "st-review", "name": "In Review", "position": 3.0},
		map[string]any{"id": "st-doing", "name": "In Progress", "position": 1.0},
	)))
	r := runWith(t, opts{api: api, key: testKey}, "issue", "create", "--team", "ENG", "--title", "x", "--state-type", "started", "--dry-run", "--json")
	if in, _ := decode(t, r.out)["input"].(map[string]any); r.code != 0 || in["stateId"] != "st-doing" {
		t.Fatalf("exit %d, %q", r.code, r.out)
	}
	if v := api.last("CreateResolve").Vars; v["stateType"] != "started" || v["withStateType"] != true || v["withState"] != false {
		t.Errorf("vars %v", v)
	}
	api = createFake(t, resolveAnswer(typedTeam()))
	r = runWith(t, opts{api: api, key: testKey}, "issue", "create", "--team", "ENG", "--title", "x", "--state-type", "triage", "--dry-run", "--json")
	if e, _ := decode(t, r.out)["error"].(string); r.code != exitFailed || !strings.Contains(e, "no workflow state of type triage") {
		t.Errorf("none of that type: exit %d, %q", r.code, r.out)
	}
	for _, args := range [][]string{{"--state-type", "doing"}, {"--state-type", "started", "--state", "Todo"}} {
		if r := runWith(t, opts{api: newFake(t), key: testKey}, append([]string{"issue", "create", "--team", "ENG", "--title", "x", "--dry-run"}, args...)...); r.code != exitUsage {
			t.Errorf("%v: exit %d", args, r.code)
		}
	}
}

func TestUpdateStateTypeIsUnchangedWhenAlreadyThere(t *testing.T) {
	t.Parallel()
	is := currentIssue() // In Progress, st-doing
	is["team"].(map[string]any)["typed"] = nodes(map[string]any{"id": "st-doing", "name": "In Progress", "position": 1.0})
	api := newFake(t).on("UpdateResolve", resolveAnswer(map[string]any{"issue": is}))
	r := runWith(t, opts{api: api, key: testKey}, "issue", "update", "ENG-5", "--state-type", "started", "--yes", "--json")
	if r.code != 0 || decode(t, r.out)["changed"] != false {
		t.Errorf("exit %d, %q", r.code, r.out)
	}
}

func TestDelegateMe(t *testing.T) {
	t.Parallel()
	for _, c := range []struct {
		app  bool
		code int
	}{{true, 0}, {false, exitFailed}} {
		api := createFake(t, resolveAnswer(map[string]any{"viewer": map[string]any{"id": "u-agent", "name": "Codex", "email": "codex@x", "app": c.app}}))
		r := runWith(t, opts{api: api, key: testKey}, "issue", "create", "--team", "ENG", "--title", "x", "--delegate", "me", "--dry-run", "--json")
		d := decode(t, r.out)
		if r.code != c.code {
			t.Errorf("app %v: exit %d, %q", c.app, r.code, r.out)
			continue
		}
		if c.app && d["input"].(map[string]any)["delegateId"] != "u-agent" {
			t.Errorf("app: input %v", d["input"])
		}
		if !c.app && !strings.Contains(d["error"].(string), "needs an app (agent) token") {
			t.Errorf("person: %q", r.out)
		}
		if v := api.last("CreateResolve").Vars; v["me"] != true || v["withDelegate"] != false {
			t.Errorf("vars %v", v)
		}
	}
	api := listFake(t, conn(""))
	runWith(t, opts{api: api, key: testKey}, "issue", "list", "--delegate", "me")
	if got := jsonOf(api.last("IssueList").Vars["filter"]); got != `{"delegate":{"isMe":{"eq":true}}}` {
		t.Errorf("list --delegate me: %s", got)
	}
}

// Estimate, due date and cycle are settable by flag, with none to clear;
// --cycle names a cycle of the issue's team.
func TestIssueEstimateDueDateCycle(t *testing.T) {
	t.Parallel()
	team := map[string]any{"teams": conn("", map[string]any{"id": "team-eng", "key": "ENG", "states": nodes(), "typed": nodes(),
		"cycles": nodes(map[string]any{"id": "cy-24", "number": 24.0})})}
	api := createFake(t, resolveAnswer(team))
	r := runWith(t, opts{api: api, key: testKey}, "issue", "create", "--team", "ENG", "--title", "x",
		"--estimate", "3", "--due-date", "2026-11-30", "--cycle", "current", "--dry-run", "--json")
	in, _ := decode(t, r.out)["input"].(map[string]any)
	if r.code != 0 || in["estimate"] != 3.0 || in["dueDate"] != "2026-11-30" || in["cycleId"] != "cy-24" {
		t.Fatalf("create: exit %d, %q", r.code, r.out)
	}
	if v := api.last("CreateResolve").Vars; jsonOf(v["cycle"]) != `{"isActive":{"eq":true}}` || v["withCycle"] != true {
		t.Errorf("vars %v", v)
	}
	r = runWith(t, opts{api: createFake(t, resolveAnswer(map[string]any{"teams": conn("", map[string]any{"id": "team-eng", "key": "ENG",
		"states": nodes(), "typed": nodes(), "cycles": nodes()})})), key: testKey},
		"issue", "create", "--team", "ENG", "--title", "x", "--cycle", "99", "--dry-run", "--json")
	if d := decode(t, r.out); r.code != exitFailed || d["code"] != "NOT_FOUND" {
		t.Errorf("no such cycle: exit %d, %q", r.code, r.out)
	}
	for _, args := range [][]string{{"--estimate", "lots"}, {"--estimate", "-1"}, {"--due-date", "tomorrow"}, {"--due-date", "2026-11-31x"}, {"--cycle", "last"}} {
		if r := runWith(t, opts{api: newFake(t), key: testKey}, append([]string{"issue", "create", "--team", "ENG", "--title", "x", "--dry-run"}, args...)...); r.code != exitUsage {
			t.Errorf("%v: exit %d", args, r.code)
		}
	}
	// Update: values as they are send nothing (Linear's estimate is a float);
	// none clears.
	is := currentIssue()
	is["estimate"], is["dueDate"], is["cycle"] = 3.0, "2026-11-30", map[string]any{"id": "cy-24"}
	is["team"].(map[string]any)["cycles"] = nodes(map[string]any{"id": "cy-24", "number": 24.0})
	for _, c := range []struct {
		args []string
		want string
	}{
		{[]string{"--estimate", "3", "--due-date", "2026-11-30", "--cycle", "24"}, ""},
		{[]string{"--estimate", "none", "--due-date", "none", "--cycle", "none"}, `{"cycleId":null,"dueDate":null,"estimate":null}`},
		{[]string{"--estimate", "5"}, `{"estimate":5}`},
	} {
		api := newFake(t).on("UpdateResolve", resolveAnswer(map[string]any{"issue": is}))
		r := runWith(t, opts{api: api, key: testKey}, append([]string{"issue", "update", "ENG-5", "--dry-run", "--json"}, c.args...)...)
		d := decode(t, r.out)
		got := ""
		if d["changed"] == true {
			got = jsonOf(d["input"])
		}
		if r.code != 0 || got != c.want {
			t.Errorf("%v: exit %d, input %s, want %s", c.args, r.code, got, c.want)
		}
	}
}
