package acceptance

import (
	"bufio"
	"bytes"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"syscall"
	"testing"
	"time"
)

// stream is a running linearctl watch, read line by line.
type stream struct {
	t     *testing.T
	cmd   *exec.Cmd
	lines chan map[string]any
	err   *bytes.Buffer
}

func startWatch(t *testing.T, api *fake, args ...string) *stream {
	t.Helper()
	dir := t.TempDir()
	cmd := exec.Command(bin, append([]string{"watch", "--json", "--batch-ms", "50"}, args...)...)
	cmd.Dir = dir
	env := []string{}
	for _, kv := range os.Environ() {
		if !strings.HasPrefix(kv, "LINEAR") {
			env = append(env, kv)
		}
	}
	cmd.Env = append(env, "LINEARCTL_HOME="+filepath.Join(dir, ".home"), "LINEARCTL_API_URL="+api.url(), "LINEAR_API_KEY="+testKey)
	out, err := cmd.StdoutPipe()
	must(t, err)
	s := &stream{t: t, cmd: cmd, lines: make(chan map[string]any, 100), err: &bytes.Buffer{}}
	cmd.Stderr = s.err
	must(t, cmd.Start())
	go func() {
		sc := bufio.NewScanner(out)
		for sc.Scan() {
			var m map[string]any
			if json.Unmarshal(sc.Bytes(), &m) != nil {
				m = map[string]any{"event": "(not JSON)", "line": sc.Text()}
			}
			s.lines <- m
		}
		close(s.lines)
	}()
	t.Cleanup(func() { _ = cmd.Process.Kill() })
	return s
}

// next is the next line with this event, skipping heartbeats unless asked.
func (s *stream) next(event string) map[string]any {
	s.t.Helper()
	deadline := time.After(8 * time.Second)
	for {
		select {
		case m, ok := <-s.lines:
			if !ok {
				s.t.Fatalf("stream ended waiting for %s; stderr %q", event, s.err.String())
			}
			if m["event"] == event {
				return m
			}
			if m["event"] != "heartbeat" {
				s.t.Logf("skipped %v", m)
			}
		case <-deadline:
			s.t.Fatalf("no %s within 8 s; stderr %q", event, s.err.String())
		}
	}
}

// stop interrupts the stream as Ctrl-C would and returns its exit code.
func (s *stream) stop() int {
	_ = s.cmd.Process.Signal(syscall.SIGINT)
	_ = s.cmd.Wait()
	return s.cmd.ProcessState.ExitCode()
}

// watchFake answers watch's queries from a set of issues that match the
// filter; tests change the set and push ids.
type watchFake struct {
	*fake
	mu       sync.Mutex
	matching map[string]bool          // ids that match the filter now
	archived map[string]bool          // ids archived
	changed  []any                    // WatchChanged nodes
	replay   []any                    // issues IssueList returns for an updatedAt filter
	sent     []map[string]interface{} // IssueList filters received
}

func newWatchFake(t *testing.T) *watchFake {
	w := &watchFake{fake: newFake(t), matching: map[string]bool{"id-ENG-1": true}, archived: map[string]bool{}}
	w.on("WatchIDs", map[string]any{"issues": conn("", map[string]any{"id": "id-ENG-1", "identifier": "ENG-1"})})
	w.onFunc("IssueList", func(vars map[string]any) (int, any, map[string]string) {
		w.mu.Lock()
		defer w.mu.Unlock()
		f := jsonOf(vars["filter"])
		nodes := []any{}
		if strings.Contains(f, "updatedAt") {
			nodes = append(nodes, w.replay...)
		} else {
			for id := range w.matching {
				if strings.Contains(f, `"`+id+`"`) {
					nodes = append(nodes, issueNode(strings.TrimPrefix(id, "id-"), "t"))
				}
			}
		}
		return 200, map[string]any{"data": map[string]any{"issues": conn("", nodes...)}}, nil
	})
	w.onFunc("WatchGone", func(vars map[string]any) (int, any, map[string]string) {
		w.mu.Lock()
		defer w.mu.Unlock()
		nodes := []any{}
		for _, id := range vars["ids"].([]any) {
			n := map[string]any{"id": id, "identifier": strings.TrimPrefix(id.(string), "id-"), "archivedAt": nil, "trashed": false}
			if w.archived[id.(string)] {
				n["archivedAt"] = "2026-10-26T00:00:00.000Z"
			}
			nodes = append(nodes, n)
		}
		return 200, map[string]any{"data": map[string]any{"issues": map[string]any{"nodes": nodes}}}, nil
	})
	w.onFunc("WatchChanged", func(map[string]any) (int, any, map[string]string) {
		w.mu.Lock()
		defer w.mu.Unlock()
		return 200, map[string]any{"data": map[string]any{"issues": map[string]any{"nodes": append([]any{}, w.changed...)}}}, nil
	})
	return w
}

// subscribedAtLeast waits until the fake has accepted n subscriptions: they
// reach it asynchronously, so a push or a drop before then hits nothing.
func (w *watchFake) subscribedAtLeast(t *testing.T, n int) {
	t.Helper()
	for deadline := time.Now().Add(5 * time.Second); w.subscribed() < n; time.Sleep(10 * time.Millisecond) {
		if time.Now().After(deadline) {
			t.Fatalf("%d subscriptions, want %d", w.subscribed(), n)
		}
	}
}

func (w *watchFake) set(f func()) {
	w.mu.Lock()
	defer w.mu.Unlock()
	f()
}

func TestWatchStreamsUpsertsAndRemoves(t *testing.T) {
	t.Parallel()
	api := newWatchFake(t)
	s := startWatch(t, api.fake, "--team", "ENG", "--heartbeat", "1", "--fields", "identifier,state")
	ready := s.next("ready")
	if ready["mode"] != "subscription" || ready["known"] != 1.0 || ready["everything"] != false {
		t.Fatalf("ready %v", ready)
	}
	api.subscribedAtLeast(t, 4) // created, updated, archived, unarchived
	if n := api.subscribed(); n != 4 {
		t.Errorf("%d subscriptions, want 4", n)
	}
	// A new issue starts matching.
	api.set(func() { api.matching["id-ENG-2"] = true })
	api.push("id-ENG-2")
	up := s.next("upsert")
	if it := up["issue"].(map[string]any); it["identifier"] != "ENG-2" || len(it) != 2 {
		t.Errorf("upsert %v", up)
	}
	if f := jsonOf(api.last("IssueList").Vars["filter"]); f != `{"and":[{"team":{"key":{"eq":"ENG"}}},{"id":{"in":["id-ENG-2"]}}]}` {
		t.Errorf("check filter %s", f)
	}
	// A known issue is archived: removed, with the reason.
	api.set(func() { delete(api.matching, "id-ENG-1"); api.archived["id-ENG-1"] = true })
	api.push("id-ENG-1")
	if rm := s.next("remove"); rm["id"] != "id-ENG-1" || rm["identifier"] != "ENG-1" || rm["reason"] != "archived" {
		t.Errorf("remove %v", rm)
	}
	// An issue that never matched changes: nothing to report.
	api.push("id-OPS-9")
	hb := s.next("heartbeat")
	if hb["mode"] != "subscription" {
		t.Errorf("heartbeat %v", hb)
	}
	if code := s.stop(); code != 130 {
		t.Errorf("exit %d", code)
	}
	for m := range s.lines {
		if m["event"] == "remove" || m["event"] == "upsert" {
			t.Errorf("unexpected %v", m)
		}
	}
}

func TestWatchReplaysSinceAndReconnects(t *testing.T) {
	t.Parallel()
	api := newWatchFake(t)
	api.set(func() {
		api.replay = []any{issueNode("ENG-3", "replayed")}
		api.changed = []any{map[string]any{"id": "id-ENG-3"}, map[string]any{"id": "id-ENG-1"}}
		delete(api.matching, "id-ENG-1") // ENG-1 changed and no longer matches
	})
	s := startWatch(t, api.fake, "--since", "2026-10-26T00:00:00Z", "--heartbeat", "1")
	if up := s.next("upsert"); up["issue"].(map[string]any)["identifier"] != "ENG-3" {
		t.Errorf("replay %v", up)
	}
	if rm := s.next("remove"); rm["id"] != "id-ENG-1" || rm["reason"] != "filter" {
		t.Errorf("replay remove %v", rm)
	}
	s.next("ready")
	api.set(func() { api.replay = nil; api.changed = nil })
	api.subscribedAtLeast(t, 4)
	api.dropWS()
	gap := s.next("gap")
	if gap["reason"] != "reconnect" || gap["mode"] != "subscription" {
		t.Errorf("gap %v", gap)
	}
	// The new subscriptions reach the fake after the gap line: wait for them.
	api.subscribedAtLeast(t, 8)
	api.set(func() { api.matching["id-ENG-4"] = true })
	api.push("id-ENG-4")
	if up := s.next("upsert"); up["issue"].(map[string]any)["identifier"] != "ENG-4" {
		t.Errorf("after reconnect %v", up)
	}
	if code := s.stop(); code != 130 {
		t.Errorf("exit %d", code)
	}
}

func TestWatchFallsBackToPolling(t *testing.T) {
	t.Parallel()
	api := newWatchFake(t)
	api.set(func() { api.noWS = true })
	s := startWatch(t, api.fake)
	if ready := s.next("ready"); ready["mode"] != "poll" {
		t.Errorf("ready %v", ready)
	}
	if !strings.Contains(s.err.String(), "polling every 1m0s") {
		t.Errorf("stderr %q", s.err.String())
	}
	s.stop()
	r := runWith(t, opts{api: api.fake, key: testKey}, "watch", "--mode", "subscription", "--json")
	if r.code != exitFailed || decode(t, r.out)["error"] == nil {
		t.Errorf("--mode subscription without subscriptions: exit %d, %q", r.code, r.out)
	}
}

func TestWatchFailuresEndTheStream(t *testing.T) {
	t.Parallel()
	api := newFake(t).onFunc("WatchIDs", func(map[string]any) (int, any, map[string]string) {
		return 400, map[string]any{"errors": []any{map[string]any{"message": "ratelimited", "extensions": map[string]any{"code": "RATELIMITED"}}}},
			map[string]string{"X-RateLimit-Requests-Remaining": "0", "X-RateLimit-Requests-Reset": "1793000000000"}
	})
	r := runWith(t, opts{api: api, key: testKey}, "watch", "--mode", "poll", "--json")
	if r.code != exitLimited || decode(t, r.out)["retryAt"] == nil {
		t.Errorf("rate-limited: exit %d, %q", r.code, r.out)
	}
	for _, args := range [][]string{{"--mode", "push"}, {"--heartbeat", "0"}, {"--poll-interval", "5"}, {"--since", "soon"}, {"--state-type", "done"}} {
		if r := runWith(t, opts{key: testKey}, append([]string{"watch"}, args...)...); r.code != exitUsage {
			t.Errorf("%v: exit %d", args, r.code)
		}
	}
}
