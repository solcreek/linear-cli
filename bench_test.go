package main

import (
	"encoding/json"
	"fmt"
	"io"
	"strings"
	"testing"
)

// benchPage is a 100-issue IssueList answer shaped like Linear's.
func benchPage() []byte {
	nodes := []any{}
	for i := range 100 {
		n := issueNode(fmt.Sprintf("ENG-%d", i+1), "Login times out after the session refresh on slow networks")
		n["project"] = map[string]any{"name": "Q4 reliability"}
		n["cycle"] = map[string]any{"number": 42}
		nodes = append(nodes, n)
	}
	b, _ := json.Marshal(map[string]any{"issues": conn("cursor-100", nodes...)})
	return b
}

// benchShow is an IssueShow answer with a long description and 100 comments.
func benchShow() []byte {
	is := issueNode("ENG-1", "Login times out")
	is["description"] = strings.Repeat("Steps to reproduce: open the app on a throttled connection and wait.\n", 60)
	cs := []any{}
	for i := range 100 {
		cs = append(cs, map[string]any{"id": fmt.Sprint(i), "body": strings.Repeat("Reproduced on staging. ", 12),
			"createdAt": "2026-10-03T00:00:00.000Z", "user": map[string]any{"name": "Bo", "email": "bo@example.com"}})
	}
	is["comments"] = nodes(cs...)
	b, _ := json.Marshal(map[string]any{"issue": is})
	return b
}

func BenchmarkListDecode(b *testing.B) {
	data := benchPage()
	b.SetBytes(int64(len(data)))
	for b.Loop() {
		var out map[string]page
		if err := json.Unmarshal(data, &out); err != nil {
			b.Fatal(err)
		}
	}
}

func BenchmarkListShape(b *testing.B) {
	data := benchPage()
	var out map[string]page
	_ = json.Unmarshal(data, &out)
	b.ResetTimer()
	for b.Loop() {
		// listDoc flattens in place; decode a fresh copy outside the timer.
		b.StopTimer()
		var fresh map[string]page
		_ = json.Unmarshal(data, &fresh)
		b.StartTimer()
		_ = listDocQuiet(fresh["issues"].Nodes, "")
	}
}

func BenchmarkListEncode(b *testing.B) {
	data := benchPage()
	var out map[string]page
	_ = json.Unmarshal(data, &out)
	doc := listDocQuiet(out["issues"].Nodes, "")
	for b.Loop() {
		enc := json.NewEncoder(io.Discard)
		enc.SetIndent("", "  ")
		if err := enc.Encode(doc); err != nil {
			b.Fatal(err)
		}
	}
}

func BenchmarkShowEndToEnd(b *testing.B) {
	data := benchShow()
	b.SetBytes(int64(len(data)))
	for b.Loop() {
		var out struct {
			Issue map[string]any `json:"issue"`
		}
		_ = json.Unmarshal(data, &out)
		enc := json.NewEncoder(io.Discard)
		enc.SetIndent("", "  ")
		_ = enc.Encode(map[string]any{"issue": flatten(out.Issue)})
	}
}

// listDocQuiet is listDoc without its stderr note.
func listDocQuiet(items []map[string]any, next string) map[string]any {
	out := make([]map[string]any, len(items))
	for i, it := range items {
		out[i] = flatten(it).(map[string]any)
	}
	return map[string]any{"items": out, "total": nil, "shown": len(out), "truncated": next != "", "nextCursor": nil}
}
