package acceptance

import (
	"bufio"
	"bytes"
	"crypto/rand"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// The live tests talk to api.linear.app with a real key and change a real
// workspace, so they run only when asked: LINEARCTL_LIVE_TEAM names a team
// whose issues may be created and archived (a sandbox), and LINEAR_API_KEY a
// key for it. Each run leaves one archived issue behind, titled
// "linearctl live test …"; linearctl has no delete.
//
//	LINEARCTL_LIVE_TEAM=SANDBOX LINEAR_API_KEY=… make live

func liveTeam(t *testing.T) string {
	t.Helper()
	team := os.Getenv("LINEARCTL_LIVE_TEAM")
	if team == "" || os.Getenv("LINEAR_API_KEY") == "" {
		t.Skip("live: set LINEARCTL_LIVE_TEAM (a sandbox team) and LINEAR_API_KEY")
	}
	return team
}

// liveEnv is the environment of a live run: the real API, the key from the
// environment, and an empty home of its own.
func liveEnv(t *testing.T, dir string) []string {
	env := []string{}
	for _, kv := range os.Environ() {
		if !strings.HasPrefix(kv, "LINEARCTL_") {
			env = append(env, kv)
		}
	}
	return append(env, "LINEARCTL_HOME="+filepath.Join(dir, ".home"))
}

// live runs linearctl against the real API and decodes its JSON answer.
func live(t *testing.T, args ...string) map[string]any {
	t.Helper()
	dir := t.TempDir()
	cmd := exec.Command(bin, append(args, "--json")...)
	cmd.Dir, cmd.Env = dir, liveEnv(t, dir)
	var out, errb bytes.Buffer
	cmd.Stdout, cmd.Stderr = &out, &errb
	if err := cmd.Run(); err != nil {
		t.Fatalf("%v: %v\n%s%s", args, err, out.String(), errb.String())
	}
	return decode(t, out.String())
}

func randomID() string {
	b := make([]byte, 16)
	_, _ = rand.Read(b)
	b[6] = b[6]&0x0f | 0x40
	b[8] = b[8]&0x3f | 0x80
	return fmt.Sprintf("%x-%x-%x-%x-%x", b[0:4], b[4:6], b[6:8], b[8:10], b[10:16])
}

// liveWatch starts linearctl watch on team against the real API.
func liveWatch(t *testing.T, team string) *stream {
	t.Helper()
	dir := t.TempDir()
	cmd := exec.Command(bin, "watch", "--team", team, "--json", "--batch-ms", "200", "--heartbeat", "5")
	cmd.Dir, cmd.Env = dir, liveEnv(t, dir)
	out, err := cmd.StdoutPipe()
	must(t, err)
	s := &stream{t: t, cmd: cmd, lines: make(chan map[string]any, 100), err: &bytes.Buffer{}}
	cmd.Stderr = s.err
	must(t, cmd.Start())
	go func() {
		sc := bufio.NewScanner(out)
		sc.Buffer(make([]byte, 1<<20), 16<<20)
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

// await is the first line with this event for which ok holds, within limit;
// other lines are skipped.
func (s *stream) await(event string, limit time.Duration, ok func(map[string]any) bool) map[string]any {
	s.t.Helper()
	deadline := time.After(limit)
	for {
		select {
		case m, open := <-s.lines:
			if !open {
				s.t.Fatalf("stream ended waiting for %s; stderr %q", event, s.err.String())
			}
			if m["event"] == event && ok(m) {
				return m
			}
		case <-deadline:
			s.t.Fatalf("no %s within %v; stderr %q", event, limit, s.err.String())
		}
	}
}

// A change made in Linear reaches watch through its subscription: an issue
// created, its title changed, then archived, each seen as it happens.
func TestLiveWatchSeesChangesAsTheyHappen(t *testing.T) {
	team := liveTeam(t)
	s := liveWatch(t, team)
	ready := s.await("ready", 60*time.Second, func(map[string]any) bool { return true })
	if ready["mode"] != "subscription" {
		t.Fatalf("ready %v: not subscribed (stderr %q)", ready, s.err.String())
	}

	id := randomID()
	title := "linearctl live test " + time.Now().UTC().Format(time.RFC3339)
	created := live(t, "issue", "create", "--team", team, "--title", title, "--id", id, "--yes")
	ident := str(created, "issue.identifier")
	t.Logf("created %s", ident)
	archived := false
	t.Cleanup(func() {
		if !archived {
			live(t, "issue", "archive", ident, "--yes")
		}
	})
	ofIssue := func(m map[string]any) bool { return str(m, "issue.identifier") == ident }
	start := time.Now()
	up := s.await("upsert", 30*time.Second, ofIssue)
	t.Logf("create seen after %v", time.Since(start).Round(time.Millisecond))
	if str(up, "issue.title") != title {
		t.Errorf("upsert %v", up)
	}

	start = time.Now()
	live(t, "issue", "update", ident, "--title", title+" (updated)", "--yes")
	s.await("upsert", 30*time.Second, func(m map[string]any) bool { return ofIssue(m) && str(m, "issue.title") == title+" (updated)" })
	t.Logf("update seen after %v", time.Since(start).Round(time.Millisecond))

	start = time.Now()
	live(t, "issue", "archive", ident, "--yes")
	archived = true
	gone := s.await("remove", 30*time.Second, func(m map[string]any) bool { return str(m, "identifier") == ident })
	t.Logf("archive seen after %v", time.Since(start).Round(time.Millisecond))
	if gone["reason"] != "archived" {
		t.Errorf("remove %v", gone)
	}
	if code := s.stop(); code != 130 {
		t.Errorf("watch exit %d, want 130; stderr %q", code, s.err.String())
	}
}
