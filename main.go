// Command linearctl: an agent-first CLI for Linear.
//
// It speaks GraphQL to api.linear.app directly (no SDK); every document it
// sends is in queries.go and checked against graphql/schema.graphql by the
// tests. Conventions: the cli plugin's audit/references/conventions.md.
package main

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/signal"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"
	"unicode"

	"github.com/spf13/cobra"
	"github.com/spf13/pflag"
)

var version = "0.1.0" // set by -ldflags "-X main.version=..." at release

// Every exit code, in help and in `describe`. Add to it; never reuse one.
const (
	exitOK      = 0
	exitFailed  = 1
	exitUsage   = 2
	exitPartial = 3
	exitBusy    = 4
	exitAuth    = 5
	exitLimited = 6
)

const exitHelp = "Exit codes: 0 ok, 1 failed, 2 usage, 3 partial, 4 busy, 5 auth (no API key, or\n" +
	"Linear rejected it), 6 rate-limited (retry after retryAt)"

// usageError marks a mistake in the invocation: exit 2, a short message.
type usageError struct{ error }

// partialError carries a result that is already printed: exit 3.
type partialError struct{ error }

// busyError: another run holds a lock this one needs; nothing changed, retry
// (exit 4).
type busyError struct{ error }

// authError: no API key, or Linear rejected it (exit 5).
type authError struct{ error }

// limitError: Linear's request or complexity budget is spent (exit 6).
// retryAt is when it resets, zero when Linear did not say.
type limitError struct {
	error
	retryAt time.Time
}

var (
	asJSON  bool
	timeout = wholeNumber{n: 30}
)

func newRoot() *cobra.Command {
	root := &cobra.Command{
		Use:   "linearctl",
		Short: "An agent-first CLI for Linear",
		Long: "An agent-first CLI for Linear: every command takes --json, every change takes\n" +
			"--dry-run or --yes, and 'linearctl describe' prints the whole surface as JSON.\n\n" +
			"The API key comes from LINEAR_API_KEY, else from 'linearctl auth login'.\n" +
			"LINEARCTL_HOME moves config and credentials under one directory.",
		Example: "  linearctl issue list --assignee me --json\n  linearctl describe",
		Version: version,
		// cobra prints the whole usage on every error, and exits 1 for a bad
		// flag; main prints a short message and exits 2 instead (rubric D3, E3).
		SilenceUsage:  true,
		SilenceErrors: true,
		// Without a RunE, cobra answers an unknown subcommand with help and
		// exit 0. This makes it an error (rubric S2).
		Args: usageArgs(cobra.NoArgs),
		RunE: func(cmd *cobra.Command, _ []string) error { return cmd.Help() },
		// Every mutating command is guarded here, so none can forget to be.
		// cobra answers --help before this runs.
		PersistentPreRunE: func(cmd *cobra.Command, _ []string) error {
			if cmd.Annotations["mutates"] == "true" {
				_, err := change(cmd)
				return err
			}
			return nil
		},
	}
	root.SetVersionTemplate("linearctl {{.Version}}\n")
	// Our own --version, so cobra adds no -v: elsewhere -v means verbose.
	root.Flags().Bool("version", false, "print the version")
	root.PersistentFlags().BoolVar(&asJSON, "json", false, "print one JSON document on stdout")
	root.PersistentFlags().Var(&timeout, "timeout", "give up on a network request after `N` seconds")
	root.SetFlagErrorFunc(func(_ *cobra.Command, err error) error { return usageError{err} })
	// Exit codes on every help page (rubric E4); children inherit it.
	root.SetHelpTemplate(root.HelpTemplate() + "\n" + exitHelp + "\nRun 'linearctl describe' for every command as JSON.\n")

	addAuth(root)
	addIssue(root)
	addComment(root)
	addLookups(root)
	addFile(root)
	addConfig(root)
	addAPI(root)

	root.AddCommand(&cobra.Command{
		Use:     "describe",
		Short:   "Print every command, flag and exit code as JSON",
		Example: "  linearctl describe",
		Args:    usageArgs(cobra.NoArgs),
		RunE:    func(*cobra.Command, []string) error { return printJSON(describe(root)) },
	})
	return root
}

// mutating gives a command --dry-run and --yes and marks it, so the root's
// PersistentPreRunE refuses it without exactly one; RunE calls change for
// which.
func mutating(c *cobra.Command) {
	c.Flags().Bool("dry-run", false, "show what would change; change nothing")
	c.Flags().Bool("yes", false, "apply the change")
	if c.Annotations == nil {
		c.Annotations = map[string]string{}
	}
	c.Annotations["mutates"] = "true"
}

// change reports whether this is a dry run. Neither flag, or both, is a
// usage error: a mutating command never acts or prompts by default.
func change(cmd *cobra.Command) (dryRun bool, err error) {
	dryRun, _ = cmd.Flags().GetBool("dry-run")
	yes, _ := cmd.Flags().GetBool("yes")
	if dryRun == yes {
		return false, usageError{fmt.Errorf("%s changes things: pass --dry-run to preview, or --yes to apply (one of them)", cmd.Name())}
	}
	return dryRun, nil
}

// plain refuses an empty argument or a control character in one (rubric A9).
func plain(what, s string) (string, error) {
	if s == "" {
		return "", usageError{fmt.Errorf("%s is empty", what)}
	}
	if strings.IndexFunc(s, unicode.IsControl) >= 0 {
		return "", usageError{fmt.Errorf("%s contains a control character", what)}
	}
	return s, nil
}

// printable makes text from files or the network safe for a terminal.
func printable(s string) string {
	return strings.Map(func(r rune) rune {
		if unicode.IsControl(r) && r != '\t' {
			return '?'
		}
		return r
	}, s)
}

// inside is the file an output path names, refused unless it is plainly
// inside root (rubric A9). Every template's inside() refuses the same
// things: a `..` part, a trailing slash, the root itself, a parent that
// resolves outside root, and a last part that is a symlink (dangling or not),
// so the write can never follow one.
func inside(root, path string) (string, error) {
	refuse := func(format string, a ...any) error {
		return fmt.Errorf("%s: "+format, append([]any{path}, a...)...)
	}
	if slices.Contains(strings.Split(filepath.ToSlash(path), "/"), "..") {
		return "", refuse("'..' is not allowed in an output path")
	}
	if strings.HasSuffix(path, "/") || strings.HasSuffix(path, string(filepath.Separator)) {
		return "", refuse("names a directory, not a file")
	}
	realRoot, err := filepath.EvalSymlinks(root)
	if err != nil {
		return "", err
	}
	full := filepath.Clean(path)
	if !filepath.IsAbs(full) {
		full = filepath.Join(root, full)
	}
	parent, err := filepath.EvalSymlinks(filepath.Dir(full))
	if err != nil {
		return "", refuse("no such directory: %s", filepath.Dir(full))
	}
	target := filepath.Join(parent, filepath.Base(full))
	rel, err := filepath.Rel(realRoot, target)
	switch {
	case err == nil && rel == ".":
		return "", refuse("is %s itself", realRoot)
	case err != nil || rel == ".." || strings.HasPrefix(rel, ".."+string(filepath.Separator)):
		return "", refuse("outside %s", realRoot)
	}
	if fi, err := os.Lstat(target); err == nil && fi.Mode()&os.ModeSymlink != 0 {
		return "", refuse("is a symlink; refusing to write through it")
	}
	return target, nil
}

// home is where config, cache or state live (conventions: State).
// LINEARCTL_HOME moves all three under one directory; tests set it. Empty when
// there is no home to put them in (HOME unset, as under cron): never the
// current directory.
func home(kind string) string {
	if dir := os.Getenv("LINEARCTL_HOME"); dir != "" {
		abs, _ := filepath.Abs(dir)
		return filepath.Join(abs, kind)
	}
	vars := map[string][2]string{
		"config": {"XDG_CONFIG_HOME", ".config"},
		"cache":  {"XDG_CACHE_HOME", ".cache"},
		"state":  {"XDG_STATE_HOME", filepath.Join(".local", "state")},
	}[kind]
	base := os.Getenv(vars[0])
	if !filepath.IsAbs(base) { // XDG says to ignore a relative path
		h, err := os.UserHomeDir()
		if err != nil || !filepath.IsAbs(h) {
			return ""
		}
		base = filepath.Join(h, vars[1])
	}
	return filepath.Join(base, "linearctl")
}

// ours is what to remove if the run is interrupted: a temp file it made
// (rubric E5). main's signal handler empties it; the kernel drops its lock.
var ours sync.Map

// withLock runs fn holding the OS's lock on dir/name.lock (conventions:
// State): flock, or LockFileEx on Windows (lock_*.go). The kernel releases it
// when the process ends, so a crash leaves nothing stale and no pid can be
// reused. The file itself stays. After 2 s of another run holding it: busy,
// exit 4.
func withLock(dir, name string, fn func() error) error {
	path, err := inside(dir, name+".lock")
	if err != nil {
		return err
	}
	f, err := os.OpenFile(path, os.O_RDWR|os.O_CREATE, 0o600)
	if err != nil {
		return fmt.Errorf("cannot lock %s: %s", path, cause(err))
	}
	defer f.Close()
	for deadline := time.Now().Add(2 * time.Second); ; time.Sleep(20 * time.Millisecond) {
		ok, err := tryLock(f)
		if err != nil {
			return fmt.Errorf("cannot lock %s: %s", path, cause(err))
		}
		if ok {
			break
		}
		if time.Now().After(deadline) {
			return busyError{fmt.Errorf("%s is locked by another run; retry", filepath.Join(dir, name))}
		}
	}
	defer unlock(f)
	return fn()
}

// replaceFile replaces dir/name with data (conventions: State): a uniquely
// named temp file beside it, given the target's mode (a new file 0600),
// renamed over it. The rename never follows a symlink; inside refused one.
func replaceFile(dir, name string, data []byte) error {
	target, err := inside(dir, name)
	if err != nil {
		return err
	}
	mode := os.FileMode(0o600)
	if fi, err := os.Stat(target); err == nil {
		mode = fi.Mode().Perm()
	}
	f, err := os.CreateTemp(dir, "."+name+".*.tmp")
	if err != nil {
		return fmt.Errorf("cannot write %s: %s", target, cause(err))
	}
	temp := f.Name()
	ours.Store(temp, true)
	defer ours.Delete(temp)
	_, err = f.Write(data)
	if cerr := f.Close(); err == nil {
		err = cerr
	}
	if err == nil {
		err = os.Chmod(temp, mode)
	}
	if err == nil {
		err = os.Rename(temp, target)
	}
	if err != nil {
		os.Remove(temp)
		return fmt.Errorf("cannot write %s: %s", target, cause(err))
	}
	return nil
}

// wholeNumber is a flag that takes base-10 digits only, as in every template:
// pflag's own Uint would read 010 as octal and take 0x3 and 1_0.
type wholeNumber struct{ n int }

func (w *wholeNumber) String() string { return strconv.Itoa(w.n) }
func (w *wholeNumber) Type() string   { return "uint" }
func (w *wholeNumber) Set(s string) error {
	if s == "" || strings.Trim(s, "0123456789") != "" {
		return fmt.Errorf("takes a whole number, got %q", s)
	}
	n, err := strconv.Atoi(s)
	if err != nil {
		return fmt.Errorf("%q is too large", s)
	}
	w.n = n
	return nil
}

// cause is an OS error without the path it repeats ("permission denied").
func cause(err error) string {
	if u := errors.Unwrap(err); u != nil {
		return u.Error()
	}
	return err.Error()
}

// usageArgs makes an argument-count error a usage error.
func usageArgs(check cobra.PositionalArgs) cobra.PositionalArgs {
	return func(cmd *cobra.Command, args []string) error {
		if err := check(cmd, args); err != nil {
			return usageError{err}
		}
		return nil
	}
}

func output(cmd *cobra.Command, v any, text string) error {
	if asJSON {
		return printJSON(v)
	}
	fmt.Fprintln(cmd.OutOrStdout(), text)
	return nil
}

func printJSON(v any) error {
	enc := json.NewEncoder(os.Stdout)
	enc.SetIndent("", "  ")
	return enc.Encode(v)
}

// describe is the same definitions as --help, as data (rubric A10), in the
// shape every template's describe prints: see references/describe.md.
func describe(c *cobra.Command) map[string]any {
	// cobra adds --help and --version only when a command runs; add them now
	// so describe lists what --help shows.
	c.InitDefaultHelpFlag()
	flags := []map[string]any{}
	add := func(f *pflag.Flag) {
		if f.Hidden {
			return
		}
		var short, def any
		if f.Shorthand != "" {
			short = f.Shorthand
		}
		kind := "string"
		switch f.Value.Type() {
		case "bool":
			kind = "boolean"
		case "int", "uint", "int64", "uint64":
			kind = "integer"
		}
		if kind != "boolean" && f.DefValue != "" {
			def = f.DefValue
		}
		flags = append(flags, map[string]any{
			"name": f.Name, "short": short, "type": kind, "default": def,
			"description": strings.ReplaceAll(f.Usage, "`", ""), // backquotes name the placeholder
		})
	}
	c.LocalFlags().VisitAll(add)
	c.InheritedFlags().VisitAll(add)
	subs := []map[string]any{}
	for _, s := range c.Commands() {
		if s.IsAvailableCommand() && s.Name() != "completion" {
			subs = append(subs, describe(s))
		}
	}
	d := map[string]any{"name": c.Name(), "summary": c.Short, "flags": flags, "commands": subs}
	if !c.HasParent() {
		d["version"] = version
		d["exitCodes"] = map[string]int{"ok": exitOK, "failed": exitFailed, "usage": exitUsage, "partial": exitPartial,
			"busy": exitBusy, "auth": exitAuth, "rateLimited": exitLimited}
		return d
	}
	// Positionals from Use ("files [DIR]"): [X] is optional.
	args := []map[string]any{}
	for _, w := range strings.Fields(c.Use)[1:] {
		name := strings.Trim(w, "[]")
		var def any
		if v, ok := c.Annotations["default."+name]; ok {
			def = v
		}
		args = append(args, map[string]any{"name": name, "required": !strings.HasPrefix(w, "["), "default": def})
	}
	examples := []string{}
	for _, line := range strings.Split(c.Example, "\n") {
		if line = strings.TrimSpace(line); line != "" {
			examples = append(examples, line)
		}
	}
	d["usage"] = c.UseLine()
	d["mutates"] = c.Annotations["mutates"] == "true"
	if c.Annotations["stdin"] == "true" { // it reads a secret from stdin
		d["stdin"] = true
	}
	d["args"] = args
	d["examples"] = examples
	return d
}

func main() {
	// cobra runs only the nearest PersistentPreRunE: a subcommand defining its
	// own would silently skip the root's mutating guard. This runs them all.
	cobra.EnableTraverseRunHooks = true
	// Ctrl-C removes a temp file this run made (rubric E5).
	signals := make(chan os.Signal, 1)
	signal.Notify(signals, os.Interrupt, syscall.SIGTERM)
	go func() {
		s := <-signals
		ours.Range(func(p, _ any) bool { os.Remove(p.(string)); return true })
		if s == os.Interrupt {
			os.Exit(130)
		}
		os.Exit(143)
	}()
	root := newRoot()
	root.SetOut(os.Stdout) // help on stdout (rubric E1)
	root.SetErr(os.Stderr)
	err := root.Execute()
	if err == nil {
		os.Exit(exitOK)
	}
	// A partial result is already printed, problems included.
	if errors.As(err, new(partialError)) {
		os.Exit(exitPartial)
	}
	fmt.Fprintf(os.Stderr, "linearctl: %v\n", err)
	usage := errors.As(err, new(usageError))
	var limited limitError
	isLimited := errors.As(err, &limited)
	if usage {
		fmt.Fprintln(os.Stderr, "Run 'linearctl --help' for usage.")
	}
	// Under --json a failure is JSON too (rubric A5). Parsing may have
	// stopped before it saw --json, so look at the arguments themselves.
	if asJSON || slices.Contains(os.Args[1:], "--json") {
		doc := map[string]any{"error": err.Error(), "usage": usage}
		if isLimited && !limited.retryAt.IsZero() {
			doc["retryAt"] = limited.retryAt.UTC().Format(time.RFC3339)
		}
		_ = printJSON(doc)
	}
	switch {
	case usage:
		os.Exit(exitUsage)
	case errors.As(err, new(busyError)):
		os.Exit(exitBusy)
	case errors.As(err, new(authError)):
		os.Exit(exitAuth)
	case isLimited:
		os.Exit(exitLimited)
	}
	os.Exit(exitFailed)
}
