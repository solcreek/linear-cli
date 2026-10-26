package acceptance

import (
	"encoding/json"
	"net"
	"net/http"
	"strings"
	"sync/atomic"
	"testing"
)

func TestServeAnswersEachLine(t *testing.T) {
	t.Parallel()
	var conns atomic.Int32
	api := listFake(t, conn("", issueNode("ENG-1", "One")))
	api.srv.Config.ConnState = func(_ net.Conn, s http.ConnState) {
		if s == http.StateNew {
			conns.Add(1)
		}
	}
	requests := strings.Join([]string{
		`{"id":1,"argv":["issue","list","--json","--fields","identifier"]}`,
		`{"id":"two","argv":["issue","list","--json"]}`,
		`{"id":3,"argv":["issue","list","--state-type","done","--json"]}`,
		`{"id":4,"argv":["issue","list"]}`,
		`{"id":5,"argv":["watch","--json"]}`,
		`{"id":6,"argv":["comment","create","ENG-1","--body-file","-","--dry-run"]}`,
		`{"id":7,"argv":[]}`,
		`not json`,
		``,
		`{"id":8,"argv":["describe"]}`,
	}, "\n") + "\n"
	r := runWith(t, opts{api: api, key: testKey, stdin: requests}, "serve", "--stdio")
	if r.code != 0 {
		t.Fatalf("exit %d, stderr %q", r.code, r.err)
	}
	var answers []map[string]any
	for _, line := range strings.Split(strings.TrimSpace(r.out), "\n") {
		var a map[string]any
		must(t, json.Unmarshal([]byte(line), &a))
		answers = append(answers, a)
	}
	if len(answers) != 9 {
		t.Fatalf("%d answers to 9 requests: %q", len(answers), r.out)
	}
	want := []struct {
		id   any
		exit float64
	}{{1.0, 0}, {"two", 0}, {3.0, exitUsage}, {4.0, 0}, {5.0, exitUsage}, {6.0, exitUsage}, {7.0, exitUsage}, {nil, exitUsage}, {8.0, 0}}
	for i, w := range want {
		if answers[i]["id"] != w.id || answers[i]["exit"] != w.exit {
			t.Errorf("answer %d: %v", i, answers[i])
		}
	}
	if items := answers[0]["result"].(map[string]any)["items"].([]any); len(items[0].(map[string]any)) != 1 {
		t.Errorf("--fields lost: %v", items)
	}
	if res := answers[2]["result"].(map[string]any); res["usage"] != true || !strings.Contains(answers[2]["stderr"].(string), "--state-type") {
		t.Errorf("usage error: %v", answers[2])
	}
	if answers[3]["result"] != nil || answers[3]["stdout"] != "ENG-1\tTodo\tAda\tOne\n" {
		t.Errorf("text: %v", answers[3])
	}
	if d := answers[8]["result"].(map[string]any); d["name"] != "linearctl" {
		t.Errorf("describe: %v", d)
	}
	// Three issue list requests reached Linear over one connection.
	if n := len(api.calls()); n != 3 {
		t.Errorf("%d requests to Linear", n)
	}
	if n := conns.Load(); n != 1 {
		t.Errorf("%d connections", n)
	}
}

func TestServeNeedsStdio(t *testing.T) {
	t.Parallel()
	if r := run(t, "serve"); r.code != exitUsage || !strings.Contains(r.err, "--stdio") {
		t.Errorf("exit %d, %q", r.code, r.err)
	}
	if r := run(t, "serve", "--stdio"); r.code != 0 || r.out != "" {
		t.Errorf("empty stdin: exit %d, %q", r.code, r.out)
	}
}
