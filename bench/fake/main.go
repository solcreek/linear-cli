// Command fake is a Linear endpoint for benchmarks, so a run measures
// linearctl and not the network: IssueList gets a 100-issue page and
// IssueShow an issue with 100 comments, shaped as bench_test.go builds them.
//
//	go run ./bench/fake -addr 127.0.0.1:8787
package main

import (
	"bytes"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"log"
	"net/http"
	"strings"
)

func issue(identifier, title string) map[string]any {
	return map[string]any{
		"id": "id-" + identifier, "identifier": identifier, "title": title, "priority": 2, "priorityLabel": "High",
		"url": "https://linear.app/acme/issue/" + identifier, "createdAt": "2026-10-01T00:00:00.000Z",
		"updatedAt": "2026-10-02T00:00:00.000Z", "state": map[string]any{"id": "s1", "name": "Todo", "type": "unstarted", "color": "#e2e2e2"},
		"assignee": map[string]any{"id": "u1", "name": "Ada", "email": "ada@example.com"}, "delegate": nil,
		"team":    map[string]any{"id": "t1", "key": "ENG"},
		"project": map[string]any{"id": "p1", "name": "Q4 reliability"}, "cycle": map[string]any{"id": "c1", "number": 42},
		"projectMilestone": nil,
		"labels":           map[string]any{"nodes": []any{map[string]any{"id": "l1", "name": "bug", "color": "#eb5757"}}},
	}
}

func listPage() []byte {
	nodes := []any{}
	for i := range 100 {
		nodes = append(nodes, issue(fmt.Sprintf("ENG-%d", i+1), "Login times out after the session refresh on slow networks"))
	}
	b, _ := json.Marshal(map[string]any{"data": map[string]any{"issues": map[string]any{
		"nodes": nodes, "pageInfo": map[string]any{"hasNextPage": true, "endCursor": "cursor-100"}}}})
	return b
}

func showPage() []byte {
	is := issue("ENG-1", "Login times out")
	is["description"] = strings.Repeat("Steps to reproduce: open the app on a throttled connection and wait.\n", 60)
	cs := []any{}
	for i := range 100 {
		cs = append(cs, map[string]any{"id": fmt.Sprint(i), "body": strings.Repeat("Reproduced on staging. ", 12),
			"createdAt": "2026-10-03T00:00:00.000Z", "url": "https://linear.app/acme/issue/ENG-1#comment-" + fmt.Sprint(i),
			"user": map[string]any{"name": "Bo", "email": "bo@example.com"}, "parent": nil})
	}
	is["comments"] = map[string]any{"nodes": cs}
	for _, k := range []string{"children", "relations", "inverseRelations", "attachments"} {
		is[k] = map[string]any{"nodes": []any{}}
	}
	b, _ := json.Marshal(map[string]any{"data": map[string]any{"issue": is}})
	return b
}

func main() {
	addr := flag.String("addr", "127.0.0.1:8787", "listen on `ADDR`")
	flag.Parse()
	answers := map[string][]byte{"IssueList": listPage(), "IssueShow": showPage()}
	http.HandleFunc("/graphql", func(w http.ResponseWriter, r *http.Request) {
		body, _ := io.ReadAll(r.Body)
		var req struct{ Query string }
		_ = json.Unmarshal(body, &req)
		for op, answer := range answers {
			if strings.HasPrefix(strings.TrimSpace(req.Query), "query "+op+"(") {
				w.Header().Set("Content-Type", "application/json")
				_, _ = io.Copy(w, bytes.NewReader(answer))
				return
			}
		}
		http.Error(w, `{"errors":[{"message":"bench fake: unknown operation"}]}`, http.StatusBadRequest)
	})
	log.Printf("bench fake on %s", *addr)
	log.Fatal(http.ListenAndServe(*addr, nil))
}
