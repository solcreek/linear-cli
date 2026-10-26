package acceptance

import (
	"slices"
	"strings"
	"testing"
)

// relFake answers RelationResolve with issue A (and its stored relations)
// and issue B.
func relFake(t *testing.T, relations, inverse []any) *fake {
	a := map[string]any{"id": "id-A", "identifier": "ENG-1",
		"relations": conn("", relations...), "inverseRelations": conn("", inverse...)}
	return newFake(t).
		on("RelationResolve", map[string]any{"a": conn("", a), "b": conn("", map[string]any{"id": "id-B", "identifier": "ENG-2"})}).
		on("RelationCreate", map[string]any{"issueRelationCreate": map[string]any{"success": true, "issueRelation": map[string]any{"id": "rel-new"}}}).
		on("RelationDelete", map[string]any{"issueRelationDelete": map[string]any{"success": true}})
}

func out(kind, id string) map[string]any {
	return map[string]any{"id": "rel-" + kind, "type": kind, "relatedIssue": map[string]any{"id": id, "identifier": "x"}}
}

func in(kind, id string) map[string]any {
	return map[string]any{"id": "rel-in-" + kind, "type": kind, "issue": map[string]any{"id": id, "identifier": "x"}}
}

func TestRelationCreate(t *testing.T) {
	t.Parallel()
	cases := []struct {
		kind              string
		relations, invers []any
		changed           bool
		issue, related    string // the input sent, when changed
	}{
		{"blocks", nil, nil, true, "id-A", "id-B"},
		{"blocked-by", nil, nil, true, "id-B", "id-A"},
		{"duplicate", nil, nil, true, "id-A", "id-B"},
		{"blocks", []any{out("blocks", "id-B")}, nil, false, "", ""},
		{"blocked-by", nil, []any{in("blocks", "id-B")}, false, "", ""},
		{"blocks", nil, []any{in("blocks", "id-B")}, true, "id-A", "id-B"}, // B blocks A is not A blocks B
		{"related", nil, []any{in("related", "id-B")}, false, "", ""},      // either direction
		{"similar", []any{out("similar", "id-B")}, nil, false, "", ""},
		{"related", []any{out("blocks", "id-B")}, nil, true, "id-A", "id-B"},
	}
	for _, c := range cases {
		api := relFake(t, c.relations, c.invers)
		r := runWith(t, opts{api: api, key: testKey}, "relation", "create", "ENG-1", c.kind, "eng-2", "--yes", "--json")
		d := decode(t, r.out)
		if r.code != 0 || d["changed"] != c.changed {
			t.Errorf("%s: exit %d, %q", c.kind, r.code, r.out)
			continue
		}
		sent := slices.Contains(api.ops(), "RelationCreate")
		if sent != c.changed {
			t.Errorf("%s: sent %v", c.kind, sent)
		}
		if sent {
			in := api.last("RelationCreate").Vars["input"].(map[string]any)
			want := map[string]string{"blocked-by": "blocks"}[c.kind]
			if want == "" {
				want = c.kind
			}
			if in["issueId"] != c.issue || in["relatedIssueId"] != c.related || in["type"] != want {
				t.Errorf("%s: input %v", c.kind, in)
			}
		}
	}
}

func TestRelationDryRunDeleteAndRefusals(t *testing.T) {
	t.Parallel()
	api := relFake(t, nil, nil)
	r := runWith(t, opts{api: api, key: testKey}, "relation", "create", "ENG-1", "blocks", "ENG-2", "--dry-run", "--json")
	if d := decode(t, r.out); r.code != 0 || d["changed"] != true || slices.Contains(api.ops(), "RelationCreate") {
		t.Errorf("dry run: exit %d, %q", r.code, r.out)
	}
	api = relFake(t, nil, []any{in("related", "id-B")})
	r = runWith(t, opts{api: api, key: testKey}, "relation", "delete", "ENG-1", "related", "ENG-2", "--yes", "--json")
	if r.code != 0 || api.last("RelationDelete").Vars["id"] != "rel-in-related" {
		t.Errorf("delete: exit %d, %q", r.code, r.out)
	}
	api = relFake(t, nil, nil)
	r = runWith(t, opts{api: api, key: testKey}, "relation", "delete", "ENG-1", "blocks", "ENG-2", "--yes", "--json")
	if d := decode(t, r.out); r.code != 0 || d["changed"] != false || slices.Contains(api.ops(), "RelationDelete") {
		t.Errorf("delete none: exit %d, %q", r.code, r.out)
	}
	api = newFake(t).on("RelationResolve", map[string]any{"a": conn("", map[string]any{"id": "id-A", "identifier": "ENG-1",
		"relations": conn(""), "inverseRelations": conn("")}), "b": conn("")})
	r = runWith(t, opts{api: api, key: testKey}, "relation", "create", "ENG-1", "blocks", "ENG-404", "--yes", "--json")
	if e, _ := decode(t, r.out)["error"].(string); r.code != exitFailed || !strings.Contains(e, "not found: issue ENG-404") {
		t.Errorf("missing: exit %d, %q", r.code, r.out)
	}
	api = newFake(t)
	for _, args := range [][]string{{"ENG-1", "blocking", "ENG-2"}, {"ENG-1", "blocks", "eng-1"}, {"ENG-1", "blocks", "nope"}} {
		if r := runWith(t, opts{api: api, key: testKey}, append([]string{"relation", "create", "--dry-run"}, args...)...); r.code != exitUsage {
			t.Errorf("%v: exit %d", args, r.code)
		}
	}
	if len(api.calls()) != 0 {
		t.Error("a refused relation reached Linear")
	}
}

func TestRelationDeleteWithoutTypeDeletesEveryRelationBetweenTheTwo(t *testing.T) {
	t.Parallel()
	rels := []any{out("blocks", "id-B"), out("related", "id-C")}
	inv := []any{in("duplicate", "id-B")}
	deleted := func(api *fake) []string {
		var ids []string
		for _, c := range api.calls() {
			if c.Op == "RelationDelete" {
				ids = append(ids, c.Vars["id"].(string))
			}
		}
		return ids
	}

	api := relFake(t, rels, inv)
	r := runWith(t, opts{api: api, key: testKey}, "relation", "delete", "ENG-1", "ENG-2", "--dry-run", "--json")
	d := decode(t, r.out)
	if r.code != 0 || d["changed"] != true || d["dryRun"] != true || len(deleted(api)) != 0 {
		t.Fatalf("dry run: exit %d, %q, deleted %v", r.code, r.out, deleted(api))
	}

	r = runWith(t, opts{api: api, key: testKey}, "relation", "delete", "ENG-1", "ENG-2", "--yes", "--json")
	d = decode(t, r.out)
	want := `[{"id":"rel-blocks","issue":"ENG-1","relatedIssue":"x","type":"blocks"},` +
		`{"id":"rel-in-duplicate","issue":"x","relatedIssue":"ENG-1","type":"duplicate"}]`
	if r.code != 0 || d["changed"] != true || jsonOf(d["relations"]) != want {
		t.Errorf("exit %d, relations %s", r.code, jsonOf(d["relations"]))
	}
	if got := strings.Join(deleted(api), ","); got != "rel-blocks,rel-in-duplicate" {
		t.Errorf("deleted %s; a relation to another issue must stay", got)
	}

	api = relFake(t, []any{out("related", "id-C")}, nil)
	r = runWith(t, opts{api: api, key: testKey}, "relation", "delete", "ENG-1", "ENG-2", "--yes", "--json")
	if d = decode(t, r.out); r.code != 0 || d["changed"] != false || jsonOf(d["relations"]) != `[]` || len(deleted(api)) != 0 {
		t.Errorf("nothing between them: exit %d, %q", r.code, r.out)
	}

	for _, args := range [][]string{{"ENG-1"}, {"ENG-1", "blocks", "ENG-2", "ENG-3"}} {
		r = runWith(t, opts{api: api, key: testKey}, append(append([]string{"relation", "delete"}, args...), "--dry-run")...)
		if r.code != exitUsage {
			t.Errorf("%v: exit %d", args, r.code)
		}
	}
	r = runWith(t, opts{api: api, key: testKey}, "relation", "create", "ENG-1", "ENG-2", "--dry-run")
	if r.code != exitUsage {
		t.Errorf("create without TYPE: exit %d", r.code)
	}
}

func TestRelationDeleteWithoutTypeReportsAFailedDeleteAsPartial(t *testing.T) {
	t.Parallel()
	api := relFake(t, []any{out("blocks", "id-B")}, []any{in("related", "id-B")}).
		onFunc("RelationDelete", func(v map[string]any) (int, any, map[string]string) {
			if v["id"] == "rel-blocks" {
				return 200, map[string]any{"errors": []any{map[string]any{"message": "Entity not found"}}}, nil
			}
			return 200, map[string]any{"data": map[string]any{"issueRelationDelete": map[string]any{"success": true}}}, nil
		})
	r := runWith(t, opts{api: api, key: testKey}, "relation", "delete", "ENG-1", "ENG-2", "--yes", "--json")
	d := decode(t, r.out)
	if r.code != exitPartial || d["changed"] != true || jsonOf(d["relations"]) != `[{"id":"rel-in-related","issue":"x","relatedIssue":"ENG-1","type":"related"}]` {
		t.Errorf("exit %d, %q", r.code, r.out)
	}
	if p, _ := d["problems"].([]any); len(p) != 1 || p[0].(map[string]any)["input"] != "rel-blocks" {
		t.Errorf("problems %v", d["problems"])
	}
}
