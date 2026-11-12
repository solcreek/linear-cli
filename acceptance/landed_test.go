package acceptance

import (
	"sync/atomic"
	"testing"
)

// A create's guard is a read, and Linear can answer a read from before a
// write it confirmed. When an earlier attempt landed and the rerun's guard
// still sees nothing, Linear refuses the id as taken; linearctl then reads
// again until the entity shows, and answers as the guard would have (#12).

// createCase is one create command: its guard, its mutation, the guard's
// answer with the entity there or not, and where the answer carries it.
type createCase struct {
	name     string
	resolve  string
	mutation string
	answer   func(existing ...any) map[string]any
	entity   any
	key      string
	args     []string
}

func createCases() []createCase {
	return []createCase{
		{"issue", "CreateResolve", "IssueCreate",
			func(existing ...any) map[string]any {
				return resolveAnswer(map[string]any{"existing": conn("", existing...)})
			},
			issueNode("ENG-77", "New"), "issue",
			[]string{"issue", "create", "--team", "ENG", "--title", "New", "--id", newIssueID}},
		{"comment", "CommentResolve", "CommentCreate",
			func(existing ...any) map[string]any {
				return map[string]any{"issue": map[string]any{"id": "id-ENG-5", "identifier": "ENG-5"}, "comments": conn("", existing...)}
			},
			commentNode(newIssueID, "Fixed"), "comment",
			[]string{"comment", "create", "ENG-5", "--body", "Fixed", "--id", newIssueID}},
		{"cycle", "CycleCreateResolve", "CycleCreate",
			func(existing ...any) map[string]any {
				return map[string]any{"teams": conn("", map[string]any{"id": "t-ENG", "key": "ENG"}), "existing": conn("", existing...)}
			},
			cycleRow(newCycleID, nil), "cycle",
			[]string{"cycle", "create", "--team", "ENG", "--starts-at", "2026-11-02", "--ends-at", "2026-11-16", "--id", newCycleID}},
		{"project", "ProjectCreateResolve", "ProjectCreate",
			func(existing ...any) map[string]any { return projectCreateResolve(existing...) },
			projectNode("q4"), "project",
			[]string{"project", "create", "--team", "ENG", "--name", "q4", "--id", newProjectID}},
	}
}

// count is how many times op was called.
func count(f *fake, op string) int {
	n := 0
	for _, o := range f.ops() {
		if o == op {
			n++
		}
	}
	return n
}

// The guard's read lags for its first `stale` answers, then shows the entity.
func laggingGuard(t *testing.T, c createCase, stale int32) *fake {
	var reads atomic.Int32
	return newFake(t).onFunc(c.resolve, func(map[string]any) (int, any, map[string]string) {
		if reads.Add(1) <= stale {
			return 200, map[string]any{"data": c.answer()}, nil
		}
		return 200, map[string]any{"data": c.answer(c.entity)}, nil
	})
}

func TestCreateRefusedForAnIDThatLandedAnswersUnchanged(t *testing.T) {
	t.Parallel()
	for _, c := range createCases() {
		t.Run(c.name, func(t *testing.T) {
			t.Parallel()
			// The guard and the first read after the refusal both lag.
			api := laggingGuard(t, c, 2).onFunc(c.mutation, linearErr("Entity already exists", "INVALID_INPUT"))
			r := runWith(t, opts{api: api, key: testKey}, append(c.args, "--yes", "--json")...)
			d := decode(t, r.out)
			if r.code != exitOK || d["changed"] != false || d["dryRun"] != false || d[c.key] == nil {
				t.Fatalf("exit %d, %q %q", r.code, r.out, r.err)
			}
			if id, _ := d["id"].(string); !uuidPattern.MatchString(id) {
				t.Errorf("id %q", id)
			}
			if got := count(api, c.mutation); got != 1 {
				t.Errorf("%s sent %d times; the create is never repeated", c.mutation, got)
			}
			if got := count(api, c.resolve); got != 3 {
				t.Errorf("%s read %d times, want the guard and two reads after the refusal", c.resolve, got)
			}
		})
	}
}

func TestCreateRefusalStandsWhenNothingHoldsTheID(t *testing.T) {
	t.Parallel()
	for _, c := range createCases() {
		t.Run(c.name, func(t *testing.T) {
			t.Parallel()
			api := newFake(t).on(c.resolve, c.answer()).onFunc(c.mutation, linearErr("Title is too long", "INVALID_INPUT"))
			r := runWith(t, opts{api: api, key: testKey}, append(c.args, "--yes", "--json")...)
			d := decode(t, r.out)
			if r.code != exitFailed || d["code"] != "LINEAR_ERROR" || d["error"] != "Linear: Title is too long" {
				t.Fatalf("exit %d, %q", r.code, r.out)
			}
			if got := count(api, c.mutation); got != 1 {
				t.Errorf("%s sent %d times", c.mutation, got)
			}
			if got := count(api, c.resolve); got != 5 {
				t.Errorf("%s read %d times, want the guard and four reads after the refusal", c.resolve, got)
			}
		})
	}
}

// Only Linear's refusal can mean the id is taken. A budget that is spent
// says nothing about it, and the caller is told when to rerun.
func TestCreateDoesNotReadAgainAfterAFailureThatIsNotARefusal(t *testing.T) {
	t.Parallel()
	for _, c := range createCases() {
		t.Run(c.name, func(t *testing.T) {
			t.Parallel()
			api := newFake(t).on(c.resolve, c.answer()).onFunc(c.mutation, linearErr("Rate limit exceeded", "RATELIMITED"))
			r := runWith(t, opts{api: api, key: testKey}, append(c.args, "--yes", "--json")...)
			if d := decode(t, r.out); r.code != exitLimited || d["code"] != "RATE_LIMITED" {
				t.Fatalf("exit %d, %q", r.code, r.out)
			}
			if got := count(api, c.resolve); got != 1 {
				t.Errorf("%s read %d times, want only the guard", c.resolve, got)
			}
		})
	}
}

// A read that fails after the refusal leaves the refusal as the answer.
func TestCreateRefusalStandsWhenTheReadAfterItFails(t *testing.T) {
	t.Parallel()
	c := createCases()[1]
	var reads atomic.Int32
	api := newFake(t).onFunc(c.resolve, func(vars map[string]any) (int, any, map[string]string) {
		if reads.Add(1) == 1 {
			return 200, map[string]any{"data": c.answer()}, nil
		}
		return linearErr("boom", "INTERNAL_ERROR")(vars)
	}).onFunc(c.mutation, linearErr("Entity already exists", "INVALID_INPUT"))
	r := runWith(t, opts{api: api, key: testKey}, append(c.args, "--yes", "--json")...)
	if d := decode(t, r.out); r.code != exitFailed || d["error"] != "Linear: Entity already exists" {
		t.Fatalf("exit %d, %q", r.code, r.out)
	}
	if got := count(api, c.resolve); got != 2 {
		t.Errorf("%s read %d times, want the guard and one failed read", c.resolve, got)
	}
}
