package main

// The rubric's must rules, as tests: build the binary once, run it against a
// fake Linear (fake_test.go). Tests run in parallel; each gets its own
// LINEARCTL_HOME, so none touches the Keychain or the user's config.

import (
	"bytes"
	"context"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"
)

var bin string

// coverDir, when LINEARCTL_COVER_DIR is set (make cover), is where the
// binary's runs write coverage, so it counts with the in-process tests.
var coverDir = os.Getenv("LINEARCTL_COVER_DIR")

func TestMain(m *testing.M) {
	dir, _ := os.MkdirTemp("", "linearctl")
	bin = filepath.Join(dir, "linearctl")
	build := []string{"build", "-o", bin}
	if coverDir != "" {
		build = append(build, "-cover")
	}
	if out, err := exec.Command("go", append(build, ".")...).CombinedOutput(); err != nil {
		panic(string(out))
	}
	code := m.Run()
	os.RemoveAll(dir)
	os.Exit(code)
}

type result struct {
	out, err string
	code     int
}

// opts is how a run differs from the default: no API key, stdin at EOF, an
// empty working directory and home, and an API URL nothing answers.
type opts struct {
	dir, home string
	api       *fake
	key       string // LINEAR_API_KEY
	stdin     string
	env       []string
}

func runWith(t *testing.T, o opts, args ...string) result {
	t.Helper()
	if o.dir == "" {
		o.dir = t.TempDir()
	}
	if o.home == "" {
		o.home = filepath.Join(o.dir, ".home")
	}
	api := "http://127.0.0.1:9/graphql" // discard: a test that forgot a fake fails fast
	if o.api != nil {
		api = o.api.url()
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	var out, errb bytes.Buffer
	cmd := exec.CommandContext(ctx, bin, args...)
	cmd.Dir = o.dir
	env := []string{}
	for _, kv := range os.Environ() {
		if !strings.HasPrefix(kv, "LINEAR") && !strings.HasPrefix(kv, "XDG_") {
			env = append(env, kv)
		}
	}
	env = append(env, "NO_COLOR=1", "LINEARCTL_HOME="+o.home, "LINEARCTL_API_URL="+api)
	if o.key != "" {
		env = append(env, "LINEAR_API_KEY="+o.key)
	}
	if coverDir != "" {
		// A directory per run: runs sharing one race to write the same
		// meta-data file and report the loss on stderr.
		d, err := os.MkdirTemp(coverDir, "run")
		must(t, err)
		env = append(env, "GOCOVERDIR="+d)
	}
	cmd.Env = append(env, o.env...)
	cmd.Stdin = strings.NewReader(o.stdin)
	cmd.Stdout, cmd.Stderr = &out, &errb
	_ = cmd.Run()
	return result{out.String(), errb.String(), cmd.ProcessState.ExitCode()}
}

func run(t *testing.T, args ...string) result { return runWith(t, opts{}, args...) }

func exists(path string) bool { _, err := os.Lstat(path); return err == nil }

func decode(t *testing.T, s string) map[string]any {
	t.Helper()
	var v map[string]any
	if err := json.Unmarshal([]byte(s), &v); err != nil {
		t.Fatalf("not one JSON document: %q", s)
	}
	return v
}

func must(t *testing.T, err error) {
	t.Helper()
	if err != nil {
		t.Fatal(err)
	}
}

// command is a leaf command as describe lists it.
type command struct {
	Path    []string
	Mutates bool
	Args    []struct {
		Name     string
		Required bool
	}
	Flags    []struct{ Name, Type string }
	Examples []string
}

// commands walks describe's tree to its leaves: every command that runs.
func commands(t *testing.T) []command {
	t.Helper()
	r := run(t, "describe")
	var d map[string]any
	must(t, json.Unmarshal([]byte(r.out), &d))
	var out []command
	var walk func(node map[string]any, path []string)
	walk = func(node map[string]any, path []string) {
		subs, _ := node["commands"].([]any)
		if len(subs) == 0 && len(path) > 0 {
			b, _ := json.Marshal(node)
			var c command
			must(t, json.Unmarshal(b, &c))
			c.Path = path
			out = append(out, c)
		}
		for _, s := range subs {
			m := s.(map[string]any)
			walk(m, append(slices.Clone(path), m["name"].(string)))
		}
	}
	walk(d, nil)
	return out
}

func TestHelpGoesToStdoutAndActsOnNothing(t *testing.T) {
	t.Parallel()
	api := newFake(t)
	cases := [][]string{{}, {"--help"}, {"-h"}, {"help", "auth"}, {"describe", "-h"}}
	for _, c := range commands(t) {
		cases = append(cases, append(slices.Clone(c.Path), "--help"), append(slices.Clone(c.Path), "-h"))
	}
	for _, args := range cases {
		r := runWith(t, opts{api: api, key: testKey}, args...)
		if r.code != 0 || !strings.Contains(r.out, "Usage") || r.err != "" {
			t.Errorf("%v: exit %d, stdout %q, stderr %q", args, r.code, r.out, r.err)
		}
		if !strings.Contains(r.out, "6 rate-limited") {
			t.Errorf("%v: no exit codes in help", args)
		}
	}
	if n := len(api.calls()); n != 0 {
		t.Errorf("help called Linear %d times", n)
	}
}

func TestHelpFits100Columns(t *testing.T) {
	t.Parallel()
	for _, c := range commands(t) {
		for _, line := range strings.Split(run(t, append(c.Path, "--help")...).out, "\n") {
			if len([]rune(line)) > 100 {
				t.Errorf("%v: %d columns: %q", c.Path, len([]rune(line)), line)
			}
		}
	}
}

func TestUsageErrorsExit2(t *testing.T) {
	t.Parallel()
	cases := [][]string{{"--zz"}, {"nope"}, {"-v"}, {"describe", "zz"}, {"--timeout", "x", "describe"}}
	// Every command: an unknown flag, and one argument too many. A mutating
	// command gets --dry-run, or the missing-flag check would answer first.
	for _, c := range commands(t) {
		base := slices.Clone(c.Path)
		for range c.Args {
			base = append(base, "ENG-1")
		}
		if c.Mutates {
			base = append(base, "--dry-run")
		}
		cases = append(cases, append(slices.Clone(base), "--zz"), append(slices.Clone(base), "extra"))
	}
	for _, args := range cases {
		r := runWith(t, opts{key: testKey}, args...)
		if r.code != exitUsage || r.out != "" || !strings.Contains(r.err, "--help") {
			t.Errorf("%v: exit %d, stdout %q, stderr %q", args, r.code, r.out, r.err)
		}
	}
}

func TestMutatingNeedsExactlyOneOfDryRunAndYes(t *testing.T) {
	t.Parallel()
	api := newFake(t)
	for _, c := range commands(t) {
		if !c.Mutates {
			continue
		}
		base := slices.Clone(c.Path)
		for range c.Args {
			base = append(base, "ENG-1")
		}
		for _, extra := range [][]string{{}, {"--dry-run", "--yes"}} {
			r := runWith(t, opts{api: api, key: testKey}, append(append(slices.Clone(base), extra...), "--json")...)
			if r.code != exitUsage || !strings.Contains(r.err, "--dry-run") || decode(t, r.out)["usage"] != true {
				t.Errorf("%v %v: exit %d, %q", c.Path, extra, r.code, r.err)
			}
		}
	}
	if n := len(api.calls()); n != 0 {
		t.Errorf("a refused command called Linear %d times", n)
	}
}

func TestJSONFailureIsJSON(t *testing.T) {
	t.Parallel()
	for _, args := range [][]string{{"--zz", "--json"}, {"--json", "nope"}, {"describe", "--zz", "--json"}} {
		r := run(t, args...)
		if r.code != exitUsage || decode(t, r.out)["usage"] != true {
			t.Errorf("%v: exit %d, %q", args, r.code, r.out)
		}
	}
}

func TestVersion(t *testing.T) {
	t.Parallel()
	if r := run(t, "--version"); r.code != 0 || !strings.HasPrefix(r.out, "linearctl ") {
		t.Errorf("exit %d, %q", r.code, r.out)
	}
}

func TestDescribe(t *testing.T) {
	t.Parallel()
	r := run(t, "describe")
	var d struct {
		Name      string
		ExitCodes map[string]int
		Flags     []struct{ Name string }
	}
	if r.code != 0 || json.Unmarshal([]byte(r.out), &d) != nil {
		t.Fatalf("exit %d, stdout %q", r.code, r.out)
	}
	want := map[string]int{"ok": 0, "failed": 1, "usage": 2, "partial": 3, "busy": 4, "auth": 5, "rateLimited": 6}
	for k, v := range want {
		if d.ExitCodes[k] != v {
			t.Errorf("exitCodes[%s] = %d, want %d", k, d.ExitCodes[k], v)
		}
	}
	var keys map[string]any
	_ = json.Unmarshal([]byte(r.out), &keys)
	for _, k := range []string{"name", "version", "summary", "exitCodes", "flags", "commands"} {
		if _, ok := keys[k]; !ok {
			t.Errorf("no %s at the top level", k)
		}
	}
	for _, c := range commands(t) {
		flags := []string{}
		for _, f := range c.Flags {
			flags = append(flags, f.Name)
		}
		if len(c.Examples) == 0 {
			t.Errorf("%v: no examples", c.Path)
		}
		if c.Mutates != (slices.Contains(flags, "dry-run") && slices.Contains(flags, "yes")) {
			t.Errorf("%v: mutates %v, flags %v", c.Path, c.Mutates, flags)
		}
		for _, f := range []string{"json", "help", "timeout"} {
			if !slices.Contains(flags, f) {
				t.Errorf("%v: no --%s", c.Path, f)
			}
		}
	}
}

func TestJSONIsCompactIntoAPipe(t *testing.T) {
	t.Parallel()
	r := run(t, "config", "show", "--json")
	if r.code != 0 || strings.Count(r.out, "\n") != 1 || strings.Contains(r.out, ": ") {
		t.Errorf("not one compact line: %q", r.out)
	}
}

// pflag reads --flag "" on a repeatable flag as no values, which would drop
// a filter silently; every repeatable flag refuses it instead.
func TestRepeatableFlagsRefuseAnEmptyValue(t *testing.T) {
	t.Parallel()
	api := newFake(t)
	for _, args := range [][]string{
		{"issue", "list", "--state", ""}, {"issue", "list", "--state-type", ""}, {"issue", "list", "--label", ""},
		{"issue", "create", "--team", "ENG", "--title", "x", "--label", "", "--dry-run"},
		{"issue", "update", "ENG-1", "--label", "", "--dry-run"},
		{"issue", "update", "ENG-1", "--add-label", "", "--dry-run"},
		{"issue", "update", "ENG-1", "--remove-label", "", "--dry-run"},
		{"project", "list", "--status", ""},
	} {
		r := runWith(t, opts{api: api, key: testKey}, append(args, "--json")...)
		if e, _ := decode(t, r.out)["error"].(string); r.code != exitUsage || !strings.Contains(e, "is empty") {
			t.Errorf("%v: exit %d, %q", args, r.code, r.out)
		}
	}
	if len(api.calls()) != 0 {
		t.Errorf("an empty filter reached Linear: %v", api.ops())
	}
}
