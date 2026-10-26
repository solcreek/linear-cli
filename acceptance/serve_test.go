package acceptance

import (
	"encoding/json"
	"net"
	"net/http"
	"reflect"
	"strings"
	"sync/atomic"
	"testing"
	"time"
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

// serveAnswers runs serve with the request lines and returns its answers,
// in the order they came.
func serveAnswers(t *testing.T, o opts, lines []string, flags ...string) []map[string]any {
	t.Helper()
	o.stdin = strings.Join(lines, "\n") + "\n"
	r := runWith(t, o, append([]string{"serve", "--stdio"}, flags...)...)
	if r.code != 0 {
		t.Fatalf("exit %d, stderr %q", r.code, r.err)
	}
	var answers []map[string]any
	for _, line := range strings.Split(strings.TrimSpace(r.out), "\n") {
		var a map[string]any
		must(t, json.Unmarshal([]byte(line), &a))
		answers = append(answers, a)
	}
	return answers
}

func TestServeRequestStdinIsTheCommandsStdin(t *testing.T) {
	t.Parallel()
	api := createFake(t, resolveAnswer(nil))
	o := opts{api: api, key: testKey}
	argv := []string{"issue", "create", "--team", "ENG", "--json-input=-", "--id", newIssueID, "--dry-run", "--json"}
	input := `{"title":"From stdin","description":"Line one\nLine two"}`
	req, _ := json.Marshal(map[string]any{"id": 1, "argv": argv, "stdin": input})
	empty, _ := json.Marshal(map[string]any{"id": 2, "argv": argv, "stdin": ""})
	answers := serveAnswers(t, o, []string{string(req), string(empty),
		`{"id":3,"argv":["issue","list","--json"],"stdin":7}`,
		`{"id":4,"argv":["auth","login","--yes","--json"]}`})
	if len(answers) != 4 {
		t.Fatalf("answers %v", answers)
	}
	// The same as the command line with that stdin.
	for i, in := range []string{input, ""} {
		o.stdin = in
		cli := runWith(t, o, argv...)
		if answers[i]["exit"] != float64(cli.code) || !reflect.DeepEqual(answers[i]["result"], decode(t, cli.out)) {
			t.Errorf("stdin %q: serve %v, command line exit %d %s", in, answers[i], cli.code, cli.out)
		}
	}
	if in := answers[0]["result"].(map[string]any)["input"].(map[string]any); in["description"] != "Line one\nLine two" {
		t.Errorf("input %v", in)
	}
	if answers[2]["exit"] != float64(exitUsage) || answers[2]["id"] != nil {
		t.Errorf("stdin that is not text: %v", answers[2])
	}
	// auth login reads the key from stdin: never from serve's own.
	if e, _ := answers[3]["stderr"].(string); answers[3]["exit"] != float64(exitUsage) || !strings.Contains(e, `no "stdin"`) {
		t.Errorf("auth login without stdin: %v", answers[3])
	}
}

func TestServeConcurrency(t *testing.T) {
	t.Parallel()
	// Each request waits until three are in flight together, or 2 s.
	var inFlight, most atomic.Int32
	api := newFake(t).onFunc("IssueList", func(map[string]any) (int, any, map[string]string) {
		n := inFlight.Add(1)
		defer inFlight.Add(-1)
		for m := most.Load(); n > m && !most.CompareAndSwap(m, n); m = most.Load() {
		}
		for deadline := time.Now().Add(2 * time.Second); most.Load() < 3 && time.Now().Before(deadline); {
			time.Sleep(5 * time.Millisecond)
		}
		return 200, map[string]any{"data": map[string]any{"issues": conn("", issueNode("ENG-1", "One"))}}, nil
	})
	var lines []string
	for _, id := range []string{"a", "b", "c"} {
		lines = append(lines, `{"id":"`+id+`","argv":["issue","list","--json"]}`)
	}
	answers := serveAnswers(t, opts{api: api, key: testKey}, lines, "--concurrency", "3")
	seen := map[any]bool{}
	for _, a := range answers {
		if a["exit"] != 0.0 {
			t.Errorf("answer %v", a)
		}
		seen[a["id"]] = true
	}
	if len(answers) != 3 || len(seen) != 3 {
		t.Errorf("answers %v", answers)
	}
	if most.Load() != 3 {
		t.Errorf("at most %d requests ran at once, want 3", most.Load())
	}
	for _, bad := range []string{"0", "33"} {
		if r := runWith(t, opts{api: api, key: testKey}, "serve", "--stdio", "--concurrency", bad); r.code != exitUsage {
			t.Errorf("--concurrency %s: exit %d", bad, r.code)
		}
	}
}
