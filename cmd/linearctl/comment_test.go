package main

import (
	"slices"
	"strings"
	"testing"
)

func commentNode(id, body string) map[string]any {
	return map[string]any{"id": id, "body": body, "createdAt": "2026-10-03T00:00:00.000Z",
		"url": "https://linear.app/acme/issue/ENG-5#comment-" + id, "user": map[string]any{"name": "Bo"}, "parent": nil}
}

func TestCommentList(t *testing.T) {
	t.Parallel()
	pages := []map[string]any{conn("c1", commentNode("a", "first")), conn("", commentNode("b", "second\nline"))}
	n := 0
	api := newFake(t).onFunc("CommentList", func(map[string]any) (int, any, map[string]string) {
		p := pages[n]
		n++
		return 200, map[string]any{"data": map[string]any{"issue": map[string]any{"comments": p}}}, nil
	})
	r := runWith(t, opts{api: api, key: testKey}, "comment", "list", "ENG-5", "--limit", "0", "--fields", "id,body", "--json")
	d := decode(t, r.out)
	items := d["items"].([]any)
	if r.code != 0 || len(items) != 2 || len(items[0].(map[string]any)) != 2 || d["truncated"] != false {
		t.Fatalf("exit %d, %q", r.code, r.out)
	}
	if v := api.calls()[1].Vars; v["id"] != "ENG-5" || v["after"] != "c1" {
		t.Errorf("second page vars %v", v)
	}
	n = 1
	r = runWith(t, opts{api: api, key: testKey}, "comment", "list", "ENG-5")
	if r.code != 0 || !strings.Contains(r.out, "— Bo, 2026-10-03T00:00:00.000Z\nsecond\nline") {
		t.Errorf("text: %q", r.out)
	}
	api = newFake(t).onFunc("CommentList", linearErr("Entity not found: Issue", "INVALID_INPUT"))
	r = runWith(t, opts{api: api, key: testKey}, "comment", "list", "ENG-404", "--json")
	if e, _ := decode(t, r.out)["error"].(string); r.code != exitFailed || !strings.Contains(e, "not found: issue ENG-404") {
		t.Errorf("missing: exit %d, %q", r.code, r.out)
	}
}

func commentFake(t *testing.T, existing ...any) *fake {
	return newFake(t).
		on("CommentResolve", map[string]any{"issue": map[string]any{"id": "id-ENG-5", "identifier": "ENG-5"}, "comments": conn("", existing...)}).
		on("CommentCreate", map[string]any{"commentCreate": map[string]any{"success": true, "comment": commentNode(newIssueID, "Fixed")}})
}

func TestCommentCreate(t *testing.T) {
	t.Parallel()
	api := commentFake(t)
	r := runWith(t, opts{api: api, key: testKey}, "comment", "create", "ENG-5", "--body", "Fixed", "--dry-run", "--json")
	d := decode(t, r.out)
	id, _ := d["id"].(string)
	if r.code != 0 || !uuidPattern.MatchString(id) || d["changed"] != true || slices.Contains(api.ops(), "CommentCreate") {
		t.Fatalf("dry run: exit %d, %q", r.code, r.out)
	}
	if in := d["input"].(map[string]any); in["issueId"] != "id-ENG-5" || in["body"] != "Fixed" || in["id"] != id {
		t.Errorf("input %v", in)
	}

	r = runWith(t, opts{api: api, key: testKey, stdin: "Fixed\nin #42\n"}, "comment", "create", "ENG-5", "--body-file", "-", "--id", newIssueID, "--yes", "--json")
	if r.code != 0 || decode(t, r.out)["changed"] != true {
		t.Fatalf("yes: exit %d, %q", r.code, r.out)
	}
	if in := api.last("CommentCreate").Vars["input"].(map[string]any); in["body"] != "Fixed\nin #42\n" || in["id"] != newIssueID {
		t.Errorf("sent %v", in)
	}

	api = commentFake(t, commentNode(newIssueID, "Fixed"))
	r = runWith(t, opts{api: api, key: testKey}, "comment", "create", "ENG-5", "--body", "Fixed", "--id", newIssueID, "--yes", "--json")
	if r.code != 0 || decode(t, r.out)["changed"] != false || slices.Contains(api.ops(), "CommentCreate") {
		t.Errorf("rerun: exit %d, %q", r.code, r.out)
	}
}

func TestCommentCreateRefusals(t *testing.T) {
	t.Parallel()
	api := newFake(t)
	for _, args := range [][]string{
		{"--body", "x", "--yes"},        // no --id
		{"--dry-run"},                   // no body
		{"--body", "  \n", "--dry-run"}, // blank body
		{"--body", "x", "--body-file", "-", "--dry-run"},
		{"--body", "x", "--id", "nope", "--dry-run"},
	} {
		r := runWith(t, opts{api: api, key: testKey}, append([]string{"comment", "create", "ENG-5", "--json"}, args...)...)
		if r.code != exitUsage {
			t.Errorf("%v: exit %d, %q", args, r.code, r.out)
		}
	}
	if len(api.calls()) != 0 {
		t.Errorf("called Linear: %v", api.ops())
	}
}

func TestCommentCreateReply(t *testing.T) {
	t.Parallel()
	api := commentFake(t)
	parent := "5F1E2A3B-4C5D-4E6F-8A9B-0C1D2E3F4A5B"
	r := runWith(t, opts{api: api, key: testKey}, "comment", "create", "ENG-5", "--body", "+1", "--parent", parent, "--id", newIssueID, "--yes", "--json")
	if in := api.last("CommentCreate").Vars["input"].(map[string]any); r.code != 0 || in["parentId"] != strings.ToLower(parent) {
		t.Errorf("exit %d, sent %v", r.code, in)
	}
	if r := runWith(t, opts{api: api, key: testKey}, "comment", "create", "ENG-5", "--body", "+1", "--parent", "c1", "--dry-run"); r.code != exitUsage {
		t.Errorf("--parent c1: exit %d", r.code)
	}
}

func TestCommentUpdate(t *testing.T) {
	t.Parallel()
	const id = "5f1e2a3b-4c5d-4e6f-8a9b-0c1d2e3f4a5b"
	api := newFake(t).on("Comment", map[string]any{"comment": commentNode(id, "old")}).
		on("CommentUpdate", map[string]any{"commentUpdate": map[string]any{"success": true, "comment": commentNode(id, "new")}})
	r := runWith(t, opts{api: api, key: testKey}, "comment", "update", id, "--body", "new", "--dry-run", "--json")
	if d := decode(t, r.out); r.code != 0 || d["changed"] != true || slices.Contains(api.ops(), "CommentUpdate") {
		t.Errorf("dry run: exit %d, %q", r.code, r.out)
	}
	r = runWith(t, opts{api: api, key: testKey, stdin: "new"}, "comment", "update", id, "--body-file", "-", "--yes", "--json")
	if c := api.last("CommentUpdate"); r.code != 0 || c.Vars["id"] != id || c.Vars["input"].(map[string]any)["body"] != "new" {
		t.Errorf("yes: exit %d, %q", r.code, r.out)
	}
	n := len(api.calls())
	r = runWith(t, opts{api: api, key: testKey}, "comment", "update", id, "--body", "old", "--yes", "--json")
	if d := decode(t, r.out); r.code != 0 || d["changed"] != false || api.ops()[n] != "Comment" || len(api.calls()) != n+1 {
		t.Errorf("same text: exit %d, %q", r.code, r.out)
	}
	for _, args := range [][]string{{"c1", "--body", "x"}, {id}, {id, "--body", " "}} {
		if r := runWith(t, opts{api: api, key: testKey}, append([]string{"comment", "update", "--dry-run"}, args...)...); r.code != exitUsage {
			t.Errorf("%v: exit %d", args, r.code)
		}
	}
}

func TestCommentDelete(t *testing.T) {
	t.Parallel()
	const id = "5f1e2a3b-4c5d-4e6f-8a9b-0c1d2e3f4a5b"
	api := newFake(t).on("Comment", map[string]any{"comment": commentNode(id, "x")}).
		on("CommentDelete", map[string]any{"commentDelete": map[string]any{"success": true}})
	r := runWith(t, opts{api: api, key: testKey}, "comment", "delete", id, "--dry-run", "--json")
	if d := decode(t, r.out); r.code != 0 || d["changed"] != true || slices.Contains(api.ops(), "CommentDelete") {
		t.Errorf("dry run: exit %d, %q", r.code, r.out)
	}
	r = runWith(t, opts{api: api, key: testKey}, "comment", "delete", id, "--yes", "--json")
	if r.code != 0 || api.last("CommentDelete").Vars["id"] != id {
		t.Errorf("yes: exit %d, %q", r.code, r.out)
	}
	api = newFake(t).onFunc("Comment", linearErr("Entity not found: Comment", "INVALID_INPUT"))
	r = runWith(t, opts{api: api, key: testKey}, "comment", "delete", id, "--yes", "--json")
	if d := decode(t, r.out); r.code != 0 || d["changed"] != false || d["found"] != false {
		t.Errorf("gone: exit %d, %q", r.code, r.out)
	}
}
