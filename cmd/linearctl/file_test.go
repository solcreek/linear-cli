package main

import (
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync/atomic"
	"testing"
)

// uploads is a fake uploads.linear.app serving /a/b/shot.png; it counts the
// requests that carried the key.
func uploads(t *testing.T) (*httptest.Server, *atomic.Int32) {
	var keyed atomic.Int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case r.Header.Get("Authorization") != testKey:
			w.WriteHeader(http.StatusUnauthorized)
		case r.URL.Path == "/a/b/shot.png":
			keyed.Add(1)
			w.Header().Set("Content-Type", "image/png")
			_, _ = w.Write([]byte("PNGDATA"))
		default:
			keyed.Add(1)
			w.WriteHeader(http.StatusNotFound)
		}
	}))
	t.Cleanup(srv.Close)
	return srv, &keyed
}

func TestFileDownloadDryRunYesRerun(t *testing.T) {
	t.Parallel()
	srv, _ := uploads(t)
	dir, _ := filepath.EvalSymlinks(t.TempDir())
	o := opts{dir: dir, key: testKey, env: []string{"LINEARCTL_UPLOADS_URL=" + srv.URL}}
	u := srv.URL + "/a/b/shot.png"
	r := runWith(t, o, "file", "download", u, "--dry-run", "--json")
	d := decode(t, r.out)
	if r.code != 0 || d["changed"] != true || d["bytes"] != 7.0 || d["contentType"] != "image/png" || exists(filepath.Join(dir, "shot.png")) {
		t.Fatalf("dry run: exit %d, %q %q", r.code, r.out, r.err)
	}
	r = runWith(t, o, "file", "download", u, "--yes", "--json")
	if b, _ := os.ReadFile(filepath.Join(dir, "shot.png")); r.code != 0 || string(b) != "PNGDATA" || decode(t, r.out)["path"] != filepath.Join(dir, "shot.png") {
		t.Fatalf("yes: exit %d, %q %q", r.code, r.out, r.err)
	}
	r = runWith(t, o, "file", "download", u, "--yes", "--json")
	if r.code != 0 || decode(t, r.out)["changed"] != false {
		t.Errorf("rerun: exit %d, %q", r.code, r.out)
	}
	must(t, os.WriteFile(filepath.Join(dir, "other.png"), []byte("mine"), 0o644))
	r = runWith(t, o, "file", "download", u, "--output", "other.png", "--yes", "--json")
	if b, _ := os.ReadFile(filepath.Join(dir, "other.png")); r.code != exitFailed || string(b) != "mine" {
		t.Errorf("other content: exit %d, %q, file %q", r.code, r.out, b)
	}
}

func TestFileDownloadOnlyFromTheUploadsHost(t *testing.T) {
	t.Parallel()
	srv, keyed := uploads(t)
	o := opts{key: testKey, env: []string{"LINEARCTL_UPLOADS_URL=" + srv.URL}}
	host := strings.TrimPrefix(srv.URL, "http://")
	for _, bad := range []string{
		"https://evil.example/a/b/shot.png", "https://" + host + "/a/b/shot.png", "http://user:pw@" + host + "/a/b/shot.png",
		srv.URL + "/", srv.URL, "uploads.linear.app/a", "",
	} {
		if r := runWith(t, o, "file", "download", bad, "--dry-run", "--json"); r.code != exitUsage {
			t.Errorf("%q: exit %d, %q", bad, r.code, r.out)
		}
	}
	// The default host is the real one.
	if r := runWith(t, opts{key: testKey}, "file", "download", srv.URL+"/a/b/shot.png", "--dry-run"); r.code != exitUsage || !strings.Contains(r.err, "https://uploads.linear.app") {
		t.Errorf("default host: exit %d, %q", r.code, r.err)
	}
	if keyed.Load() != 0 {
		t.Error("sent the key for a refused URL")
	}
}

func TestFileDownloadOutputStaysInside(t *testing.T) {
	t.Parallel()
	srv, _ := uploads(t)
	outside := t.TempDir()
	dir, _ := filepath.EvalSymlinks(t.TempDir())
	must(t, os.Symlink(filepath.Join(outside, "x"), filepath.Join(dir, "link")))
	o := opts{dir: dir, key: testKey, env: []string{"LINEARCTL_UPLOADS_URL=" + srv.URL}}
	for _, out := range []string{"../x", filepath.Join(outside, "x"), "link", "sub/", "."} {
		r := runWith(t, o, "file", "download", srv.URL+"/a/b/shot.png", "--output", out, "--yes", "--json")
		if r.code != exitFailed {
			t.Errorf("%q: exit %d, %q", out, r.code, r.out)
		}
	}
	if exists(filepath.Join(outside, "x")) {
		t.Error("wrote outside the current directory")
	}
}

func TestFileDownloadFailures(t *testing.T) {
	t.Parallel()
	srv, _ := uploads(t)
	env := []string{"LINEARCTL_UPLOADS_URL=" + srv.URL}
	r := runWith(t, opts{key: testKey, env: env}, "file", "download", srv.URL+"/a/b/gone.png", "--yes", "--json")
	if e, _ := decode(t, r.out)["error"].(string); r.code != exitFailed || !strings.Contains(e, "not found: "+srv.URL+"/a/b/gone.png") {
		t.Errorf("404: exit %d, %q", r.code, r.out)
	}
	r = runWith(t, opts{key: "lin_api_wrong00000000000000", env: env}, "file", "download", srv.URL+"/a/b/shot.png", "--yes", "--json")
	if r.code != exitAuth {
		t.Errorf("401: exit %d, %q", r.code, r.out)
	}
}
