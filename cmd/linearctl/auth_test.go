package main

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func viewerAnswer() map[string]any {
	return map[string]any{"viewer": map[string]any{"id": "u1", "name": "Ada", "email": "ada@example.com"},
		"organization": map[string]any{"name": "Acme", "urlKey": "acme"}}
}

// noKey fails if the key shows up anywhere in a result (conventions: Secrets).
func noKey(t *testing.T, r result, key string) {
	t.Helper()
	if strings.Contains(r.out, key) || strings.Contains(r.err, key) {
		t.Errorf("the key leaked: stdout %q stderr %q", r.out, r.err)
	}
}

func TestLoginDryRunYesRerun(t *testing.T) {
	t.Parallel()
	api := newFake(t).on("Viewer", viewerAnswer())
	dir := t.TempDir()
	creds := filepath.Join(dir, ".home", "config", "credentials.json")
	o := opts{dir: dir, api: api, stdin: testKey + "\n"}

	r := runWith(t, o, "auth", "login", "--dry-run", "--json")
	d := decode(t, r.out)
	if r.code != 0 || d["changed"] != true || d["dryRun"] != true || d["store"] != "file" || exists(creds) {
		t.Fatalf("dry run: exit %d, %q, stored %v", r.code, r.out, exists(creds))
	}
	noKey(t, r, testKey)

	r = runWith(t, o, "auth", "login", "--yes", "--json")
	if r.code != 0 || decode(t, r.out)["changed"] != true {
		t.Fatalf("yes: exit %d, %q %q", r.code, r.out, r.err)
	}
	noKey(t, r, testKey)
	fi, err := os.Stat(creds)
	must(t, err)
	if fi.Mode().Perm() != 0o600 {
		t.Errorf("credentials mode %v", fi.Mode().Perm())
	}

	r = runWith(t, o, "auth", "login", "--yes", "--json")
	if r.code != 0 || decode(t, r.out)["changed"] != false {
		t.Errorf("rerun: exit %d, %q", r.code, r.out)
	}

	// The stored key is used when LINEAR_API_KEY is not set.
	r = runWith(t, opts{dir: dir, api: api}, "auth", "status", "--json")
	if d := decode(t, r.out); r.code != 0 || d["source"] != "file" || d["user"].(map[string]any)["name"] != "Ada" {
		t.Errorf("status: exit %d, %q", r.code, r.out)
	}
	noKey(t, r, testKey)
}

func TestLoginRefusesABadKeyAndStoresNothing(t *testing.T) {
	t.Parallel()
	api := newFake(t).on("Viewer", viewerAnswer())
	dir := t.TempDir()
	creds := filepath.Join(dir, ".home", "config", "credentials.json")
	cases := []struct {
		stdin string
		code  int
	}{
		{"", exitUsage},   // empty pipe
		{"\n", exitUsage}, // only a newline
		{"lin_api_has space000000000", exitUsage},      // characters a key never has
		{"short", exitUsage},                           // too short
		{"lin_api_wrong00000000000000", exitAuth},      // Linear rejects it
		{"lin_api_wrong00000000000000\n\n", exitUsage}, // only one newline is dropped
	}
	for _, c := range cases {
		r := runWith(t, opts{dir: dir, api: api, stdin: c.stdin}, "auth", "login", "--yes", "--json")
		if r.code != c.code || decode(t, r.out)["error"] == nil {
			t.Errorf("%q: exit %d, %q", c.stdin, r.code, r.out)
		}
		if c.stdin != "" && strings.TrimSpace(c.stdin) != "" {
			noKey(t, r, strings.TrimSpace(c.stdin))
		}
	}
	if exists(creds) {
		t.Error("stored a refused key")
	}
}

func TestEnvKeyWinsOverTheStoredKey(t *testing.T) {
	t.Parallel()
	api := newFake(t).on("Viewer", viewerAnswer())
	dir := t.TempDir()
	must(t, os.MkdirAll(filepath.Join(dir, ".home", "config"), 0o700))
	must(t, os.WriteFile(filepath.Join(dir, ".home", "config", "credentials.json"), []byte(`{"apiKey":"lin_api_stale0000000000000000"}`), 0o600))
	r := runWith(t, opts{dir: dir, api: api, key: testKey}, "auth", "status", "--json")
	if r.code != 0 || decode(t, r.out)["source"] != "env" {
		t.Errorf("exit %d, %q", r.code, r.out)
	}
	// Without the env key, the stale stored key is rejected: exit 5.
	r = runWith(t, opts{dir: dir, api: api}, "auth", "status", "--json")
	if r.code != exitAuth || !strings.Contains(decode(t, r.out)["error"].(string), "auth login") {
		t.Errorf("stale key: exit %d, %q", r.code, r.out)
	}
}

func TestNoKeyIsAnAuthError(t *testing.T) {
	t.Parallel()
	api := newFake(t)
	r := runWith(t, opts{api: api}, "auth", "status", "--json")
	if e, _ := decode(t, r.out)["error"].(string); r.code != exitAuth || !strings.Contains(e, "LINEAR_API_KEY") {
		t.Errorf("exit %d, %q", r.code, r.out)
	}
	if len(api.calls()) != 0 {
		t.Error("called Linear without a key")
	}
}

func TestStatusReportsTheRateLimit(t *testing.T) {
	t.Parallel()
	api := newFake(t).onFunc("Viewer", func(map[string]any) (int, any, map[string]string) {
		return 200, map[string]any{"data": viewerAnswer()}, map[string]string{
			"X-RateLimit-Requests-Remaining": "2499", "X-RateLimit-Requests-Limit": "2500", "X-RateLimit-Requests-Reset": "1793000000000"}
	})
	r := runWith(t, opts{api: api, key: testKey}, "auth", "status", "--json")
	req, _ := decode(t, r.out)["rateLimit"].(map[string]any)["requests"].(map[string]any)
	if r.code != 0 || req["remaining"] != 2499.0 || req["resetAt"] == nil {
		t.Errorf("exit %d, %q", r.code, r.out)
	}
	r = runWith(t, opts{api: api, key: testKey}, "auth", "status")
	if r.code != 0 || !strings.Contains(r.out, "2499 requests left") {
		t.Errorf("text: %q", r.out)
	}
}

func TestRateLimitedIsExit6WithRetryAt(t *testing.T) {
	t.Parallel()
	api := newFake(t).onFunc("Viewer", func(map[string]any) (int, any, map[string]string) {
		return 400, map[string]any{"errors": []any{map[string]any{"message": "ratelimited", "extensions": map[string]any{"code": "RATELIMITED"}}}},
			map[string]string{"X-RateLimit-Requests-Remaining": "0", "X-RateLimit-Requests-Reset": "1793000000000"}
	})
	r := runWith(t, opts{api: api, key: testKey}, "auth", "status", "--json")
	if r.code != exitLimited || decode(t, r.out)["retryAt"] != "2026-10-26T07:33:20Z" {
		t.Errorf("exit %d, %q", r.code, r.out)
	}
}

func TestLogoutDryRunYesRerun(t *testing.T) {
	t.Parallel()
	api := newFake(t).on("Viewer", viewerAnswer())
	dir := t.TempDir()
	creds := filepath.Join(dir, ".home", "config", "credentials.json")
	runWith(t, opts{dir: dir, api: api, stdin: testKey}, "auth", "login", "--yes")
	if !exists(creds) {
		t.Fatal("login stored nothing")
	}
	r := runWith(t, opts{dir: dir}, "auth", "logout", "--dry-run", "--json")
	if r.code != 0 || decode(t, r.out)["changed"] != true || !exists(creds) {
		t.Errorf("dry run: exit %d, %q", r.code, r.out)
	}
	r = runWith(t, opts{dir: dir}, "auth", "logout", "--yes", "--json")
	if r.code != 0 || decode(t, r.out)["changed"] != true || exists(creds) {
		t.Errorf("yes: exit %d, %q", r.code, r.out)
	}
	r = runWith(t, opts{dir: dir}, "auth", "logout", "--yes", "--json")
	if r.code != 0 || decode(t, r.out)["changed"] != false {
		t.Errorf("rerun: exit %d, %q", r.code, r.out)
	}
}

func TestDescribeSaysLoginReadsStdin(t *testing.T) {
	t.Parallel()
	var d struct {
		Commands []struct {
			Name     string
			Commands []map[string]any
		}
	}
	must(t, json.Unmarshal([]byte(run(t, "describe").out), &d))
	for _, c := range d.Commands {
		for _, s := range c.Commands {
			if want := c.Name == "auth" && s["name"] == "login"; (s["stdin"] == true) != want {
				t.Errorf("%s %s: stdin %v", c.Name, s["name"], s["stdin"])
			}
		}
	}
}
