package acceptance

import (
	"slices"
	"strings"
	"sync/atomic"
	"testing"
)

const newRelationID = "9c1e2a3b-4c5d-4e6f-8a9b-0c1d2e3f4a5b"

// --id is the id Linear gives the relation, so Linear itself refuses a
// second create: nothing in a request without one lets it (#12).
func TestRelationCreateSendsTheChosenID(t *testing.T) {
	t.Parallel()
	api := relFake(t, nil, nil)
	r := runWith(t, opts{api: api, key: testKey}, "relation", "create", "ENG-1", "blocks", "ENG-2", "--dry-run", "--json")
	d := decode(t, r.out)
	id, _ := d["id"].(string)
	if r.code != 0 || !uuidPattern.MatchString(id) || d["changed"] != true || slices.Contains(api.ops(), "RelationCreate") {
		t.Fatalf("dry run: exit %d, %q", r.code, r.out)
	}
	r = runWith(t, opts{api: api, key: testKey}, "relation", "create", "ENG-1", "blocks", "ENG-2", "--dry-run", "--id", newRelationID)
	if want := "would create: ENG-1 blocks ENG-2\nid: " + newRelationID + " (pass --id " + newRelationID + " --yes)\n"; r.out != want {
		t.Errorf("dry run text %q, want %q", r.out, want)
	}

	r = runWith(t, opts{api: api, key: testKey}, "relation", "create", "ENG-1", "blocked-by", "ENG-2",
		"--id", strings.ToUpper(newRelationID), "--yes", "--json")
	in := api.last("RelationCreate").Vars["input"].(map[string]any)
	if d := decode(t, r.out); r.code != 0 || d["changed"] != true || d["id"] != newRelationID || in["id"] != newRelationID ||
		in["issueId"] != "id-B" || str(d, "relation.id") != "rel-new" {
		t.Errorf("yes: exit %d, %q, input %v", r.code, r.out, in)
	}

	// Without --id nothing is sent as one, and the answer has none: as before.
	r = runWith(t, opts{api: api, key: testKey}, "relation", "create", "ENG-1", "blocks", "ENG-2", "--yes", "--json")
	in = api.last("RelationCreate").Vars["input"].(map[string]any)
	if _, has := in["id"]; has || r.code != 0 {
		t.Errorf("no --id: exit %d, input %v", r.code, in)
	}
	if _, has := decode(t, r.out)["id"]; has {
		t.Errorf("no --id: answered %q", r.out)
	}

	// A relation that is already there needs no id, and reports none.
	api = relFake(t, []any{out("blocks", "id-B")}, nil)
	r = runWith(t, opts{api: api, key: testKey}, "relation", "create", "ENG-1", "blocks", "ENG-2", "--id", newRelationID, "--yes", "--json")
	d = decode(t, r.out)
	if _, has := d["id"]; has || r.code != 0 || d["changed"] != false || str(d, "relation.id") != "rel-blocks" || len(api.calls()) != 1 {
		t.Errorf("exists: exit %d, %q", r.code, r.out)
	}

	n := len(api.calls())
	for _, args := range [][]string{
		{"relation", "create", "ENG-1", "blocks", "ENG-2", "--id", "nope", "--dry-run"},
		{"relation", "delete", "ENG-1", "blocks", "ENG-2", "--id", newRelationID, "--dry-run"}, // delete takes none
	} {
		if r := runWith(t, opts{api: api, key: testKey}, args...); r.code != exitUsage {
			t.Errorf("%v: exit %d, %q", args, r.code, r.err)
		}
	}
	if len(api.calls()) != n {
		t.Errorf("a refused invocation called Linear: %v", api.ops())
	}
}

// relLagging answers RelationResolve without the relation for its first
// `stale` reads, then with it; RelationCreate is refused, as for a taken id.
func relLagging(t *testing.T, stale int32, relation any) *fake {
	var reads atomic.Int32
	return newFake(t).onFunc("RelationResolve", func(map[string]any) (int, any, map[string]string) {
		var relations []any
		if reads.Add(1) > stale {
			relations = []any{relation}
		}
		a := map[string]any{"id": "id-A", "identifier": "ENG-1", "relations": conn("", relations...), "inverseRelations": conn("")}
		return 200, map[string]any{"data": map[string]any{"a": conn("", a), "b": conn("", map[string]any{"id": "id-B", "identifier": "ENG-2"})}}, nil
	}).onFunc("RelationCreate", linearErr("Entity already exists", "INVALID_INPUT"))
}

// The first attempt landed and its answer was lost; the rerun's read lags,
// so it creates again and Linear refuses the id. The relation then shows.
func TestRelationCreateRefusedForAnIDThatLandedAnswersUnchanged(t *testing.T) {
	t.Parallel()
	landed := map[string]any{"id": newRelationID, "type": "blocks", "relatedIssue": map[string]any{"id": "id-B", "identifier": "ENG-2"}}
	api := relLagging(t, 2, landed)
	r := runWith(t, opts{api: api, key: testKey}, "relation", "create", "ENG-1", "blocks", "ENG-2", "--id", newRelationID, "--yes", "--json")
	d := decode(t, r.out)
	if r.code != 0 || d["changed"] != false || str(d, "relation.id") != newRelationID || d["id"] != newRelationID {
		t.Fatalf("exit %d, %q %q", r.code, r.out, r.err)
	}
	if got := count(api, "RelationCreate"); got != 1 {
		t.Errorf("RelationCreate sent %d times; the create is never repeated", got)
	}
	if got := count(api, "RelationResolve"); got != 3 {
		t.Errorf("RelationResolve read %d times, want the guard and two reads after the refusal", got)
	}
	if r := runWith(t, opts{api: relLagging(t, 1, landed), key: testKey}, "relation", "create", "ENG-1", "blocks", "ENG-2", "--id", newRelationID, "--yes"); r.out != "exists: ENG-1 blocks ENG-2\n" {
		t.Errorf("text %q", r.out)
	}
}

func TestRelationCreateRefusalStandsWhenNoRelationShows(t *testing.T) {
	t.Parallel()
	// A relation of another type between the two is not the one asked for.
	other := map[string]any{"id": "rel-dup", "type": "duplicate", "relatedIssue": map[string]any{"id": "id-B", "identifier": "ENG-2"}}
	api := relLagging(t, 0, other)
	r := runWith(t, opts{api: api, key: testKey}, "relation", "create", "ENG-1", "blocks", "ENG-2", "--id", newRelationID, "--yes", "--json")
	if d := decode(t, r.out); r.code != exitFailed || d["code"] != "LINEAR_ERROR" || d["error"] != "Linear: Entity already exists" {
		t.Fatalf("exit %d, %q", r.code, r.out)
	}
	if got := count(api, "RelationResolve"); got != 5 {
		t.Errorf("RelationResolve read %d times, want the guard and four reads after the refusal", got)
	}
	if got := count(api, "RelationCreate"); got != 1 {
		t.Errorf("RelationCreate sent %d times", got)
	}
}
