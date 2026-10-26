package acceptance

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestConfigSetDryRunYesRerun(t *testing.T) {
	t.Parallel()
	dir := t.TempDir()
	file := filepath.Join(dir, ".home", "config", "config.json")
	o := opts{dir: dir}
	r := runWith(t, o, "config", "set", "default-team", "eng", "--dry-run", "--json")
	if d := decode(t, r.out); r.code != 0 || d["changed"] != true || d["value"] != "ENG" || exists(file) {
		t.Fatalf("dry run: exit %d, %q", r.code, r.out)
	}
	r = runWith(t, o, "config", "set", "default-team", "eng", "--yes", "--json")
	if r.code != 0 || decode(t, r.out)["changed"] != true {
		t.Fatalf("yes: exit %d, %q %q", r.code, r.out, r.err)
	}
	fi, err := os.Stat(file)
	must(t, err)
	if fi.Mode().Perm() != 0o600 {
		t.Errorf("mode %v", fi.Mode().Perm())
	}
	r = runWith(t, o, "config", "set", "default-team", "ENG", "--yes", "--json")
	if r.code != 0 || decode(t, r.out)["changed"] != false {
		t.Errorf("rerun: exit %d, %q", r.code, r.out)
	}
	r = runWith(t, o, "config", "show", "--json")
	if d := decode(t, r.out); r.code != 0 || d["settings"].(map[string]any)["default-team"] != "ENG" || d["path"] != file {
		t.Errorf("show: exit %d, %q", r.code, r.out)
	}
	if r := runWith(t, o, "config", "show"); r.out != "default-team = ENG\n" {
		t.Errorf("show text: %q", r.out)
	}
}

func TestConfigRefusals(t *testing.T) {
	t.Parallel()
	dir := t.TempDir()
	for _, args := range [][]string{
		{"config", "set", "color", "red", "--yes"},
		{"config", "set", "default-team", "E N G", "--yes"},
		{"config", "set", "default-team", "", "--yes"},
	} {
		if r := runWith(t, opts{dir: dir}, args...); r.code != exitUsage {
			t.Errorf("%v: exit %d %q", args, r.code, r.err)
		}
	}
	if exists(filepath.Join(dir, ".home", "config", "config.json")) {
		t.Error("a refused set wrote config")
	}
}

func TestConfigThatIsNotJSONIsNamed(t *testing.T) {
	t.Parallel()
	dir := t.TempDir()
	file := filepath.Join(dir, ".home", "config", "config.json")
	must(t, os.MkdirAll(filepath.Dir(file), 0o700))
	must(t, os.WriteFile(file, []byte("{oops"), 0o600))
	for _, args := range [][]string{{"config", "show"}, {"config", "set", "default-team", "ENG", "--yes"}, {"issue", "list"}} {
		r := runWith(t, opts{dir: dir, key: testKey}, args...)
		if r.code != exitFailed || !strings.Contains(r.err, "config.json is not valid JSON") {
			t.Errorf("%v: exit %d, %q", args, r.code, r.err)
		}
	}
	if b, _ := os.ReadFile(file); string(b) != "{oops" {
		t.Error("overwrote a config it could not read")
	}
}

func TestConfigSetUnderAHeldLockIsBusy(t *testing.T) {
	t.Parallel()
	dir := t.TempDir()
	cfg := filepath.Join(dir, ".home", "config")
	must(t, os.MkdirAll(cfg, 0o700))
	held, err := os.OpenFile(filepath.Join(cfg, "config.json.lock"), os.O_RDWR|os.O_CREATE, 0o600)
	must(t, err)
	if ok, err := tryLock(held); !ok || err != nil {
		t.Fatalf("could not take the lock: %v", err)
	}
	r := runWith(t, opts{dir: dir}, "config", "set", "default-team", "ENG", "--yes", "--json")
	unlock(held)
	held.Close()
	if r.code != exitBusy || exists(filepath.Join(cfg, "config.json")) {
		t.Errorf("exit %d, %q", r.code, r.out)
	}
}
