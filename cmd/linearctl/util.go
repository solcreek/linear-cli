package main

import (
	"crypto/rand"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/url"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strings"

	"github.com/spf13/cobra"
)

var (
	identPattern = regexp.MustCompile(`^[A-Za-z][A-Za-z0-9_]{0,19}-[0-9]{1,9}$`)
	uuidPattern  = regexp.MustCompile(`^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`)
	teamPattern  = regexp.MustCompile(`^[A-Za-z0-9_]{1,20}$`)
)

// issueRef is an ISSUE argument as Linear's issue(id:) takes it: ENG-123, a
// UUID, or a linear.app issue URL (rubric A9).
func issueRef(what, s string) (string, error) {
	if _, err := plain(what, s); err != nil {
		return "", err
	}
	if strings.HasPrefix(s, "https://") {
		u, err := url.Parse(s)
		parts := []string{}
		if err == nil {
			parts = strings.Split(strings.Trim(u.Path, "/"), "/")
		}
		if err != nil || u.Host != "linear.app" || len(parts) < 3 || parts[1] != "issue" || !identPattern.MatchString(parts[2]) {
			return "", usageError{fmt.Errorf("%s: not a linear.app issue URL: %s", what, s)}
		}
		return strings.ToUpper(parts[2]), nil
	}
	if identPattern.MatchString(s) {
		return strings.ToUpper(s), nil
	}
	if uuidPattern.MatchString(s) {
		return strings.ToLower(s), nil
	}
	return "", usageError{fmt.Errorf("%s must be an identifier like ENG-123, a UUID, or a linear.app issue URL; got %q", what, s)}
}

// issueFilter finds one issue by ref without failing when there is none:
// issue(id:) answers a missing issue with an error for the whole query.
func issueFilter(ref string) map[string]any {
	if uuidPattern.MatchString(ref) {
		return map[string]any{"id": map[string]any{"eq": ref}}
	}
	team, number, _ := strings.Cut(ref, "-")
	var n int
	fmt.Sscan(number, &n)
	return map[string]any{"team": map[string]any{"key": map[string]any{"eqIgnoreCase": team}}, "number": map[string]any{"eq": n}}
}

// newID is a random UUID v4, the form Linear takes for a client-chosen id.
func newID() string {
	var b [16]byte
	_, _ = rand.Read(b[:])
	b[6] = b[6]&0x0f | 0x40
	b[8] = b[8]&0x3f | 0x80
	return fmt.Sprintf("%x-%x-%x-%x-%x", b[0:4], b[4:6], b[6:8], b[8:10], b[10:16])
}

// teamFlag is --team, else LINEARCTL_TEAM, else default-team in config; ""
// when none is set.
func teamFlag(cmd *cobra.Command) (string, error) {
	teams, err := teamsFlag(cmd)
	if len(teams) == 0 || err != nil {
		return "", err
	}
	return teams[0], nil
}

// teamsFlag is --team, which issue list takes repeatedly, else the single
// default from LINEARCTL_TEAM or config; none when nothing is set.
func teamsFlag(cmd *cobra.Command) ([]string, error) {
	var given []string
	if f := cmd.Flags().Lookup("team"); f != nil && f.Value.Type() == "stringArray" {
		var err error
		if given, err = values(cmd, "team"); err != nil {
			return nil, err
		}
	} else if t, _ := cmd.Flags().GetString("team"); t != "" {
		given = []string{t}
	}
	for i, t := range given {
		if !teamPattern.MatchString(t) {
			return nil, usageError{fmt.Errorf("--team must be a team key like ENG; got %q", t)}
		}
		given[i] = strings.ToUpper(t)
	}
	if len(given) > 0 {
		return given, nil
	}
	team, err := defaultTeam()
	if team == "" || err != nil {
		return nil, err
	}
	return []string{team}, nil
}

// defaultTeam is LINEARCTL_TEAM, else default-team in config.
func defaultTeam() (string, error) {
	team, from := os.Getenv("LINEARCTL_TEAM"), "LINEARCTL_TEAM"
	if team == "" {
		cfg, err := readConfig()
		if err != nil {
			return "", err
		}
		team, from = cfg["default-team"], "default-team in config"
	}
	if team != "" && !teamPattern.MatchString(team) {
		return "", usageError{fmt.Errorf("%s must be a team key like ENG; got %q", from, team)}
	}
	return strings.ToUpper(team), nil
}

// values is a repeatable flag's values, each checked by plain. pflag reads
// --label "" as no values at all, which would drop the filter and widen
// the result without a word; that is a usage error instead.
func values(cmd *cobra.Command, name string) ([]string, error) {
	vs, _ := cmd.Flags().GetStringArray(name)
	if cmd.Flags().Changed(name) && len(vs) == 0 {
		return nil, usageError{fmt.Errorf("--%s is empty", name)}
	}
	for _, v := range vs {
		if _, err := plain("--"+name, v); err != nil {
			return nil, err
		}
	}
	return vs, nil
}

// readConfig is config.json, empty when there is none.
func readConfig() (map[string]string, error) {
	cfg := map[string]string{}
	dir := home("config")
	if dir == "" {
		return cfg, nil
	}
	path := filepath.Join(dir, "config.json")
	data, err := os.ReadFile(path)
	if errors.Is(err, os.ErrNotExist) {
		return cfg, nil
	}
	if err != nil {
		return nil, fmt.Errorf("cannot read %s: %s", path, cause(err))
	}
	if err := json.Unmarshal(data, &cfg); err != nil {
		return nil, fmt.Errorf("%s is not valid JSON: %s", path, err)
	}
	return cfg, nil
}

// text is the value of --NAME or --NAME-file (PATH, or - for stdin); set is
// false when neither was given.
func text(cmd *cobra.Command, name string) (value string, set bool, err error) {
	value, _ = cmd.Flags().GetString(name)
	file, _ := cmd.Flags().GetString(name + "-file")
	given, fileGiven := cmd.Flags().Changed(name), cmd.Flags().Changed(name+"-file")
	if given && fileGiven {
		return "", false, usageError{fmt.Errorf("pass --%s or --%s-file, not both", name, name)}
	}
	if !fileGiven {
		return value, given, nil
	}
	data, err := readInput("--"+name+"-file", file)
	return string(data), true, err
}

// readInput reads PATH, or stdin for "-". Stdin from a terminal is refused:
// the command would otherwise wait for input nobody knows to type.
func readInput(what, path string) ([]byte, error) {
	if path == "-" {
		if isTerminal(os.Stdin) {
			return nil, usageError{fmt.Errorf("%s -: pipe the input; stdin is a terminal", what)}
		}
		return io.ReadAll(io.LimitReader(os.Stdin, 16<<20))
	}
	if _, err := plain(what, path); err != nil {
		return nil, err
	}
	data, err := os.ReadFile(path)
	switch {
	case errors.Is(err, os.ErrNotExist):
		return nil, fmt.Errorf("no such file: %s", path)
	case err != nil:
		return nil, fmt.Errorf("cannot read %s: %s", path, cause(err))
	}
	return data, nil
}

func isTerminal(f *os.File) bool {
	fi, err := f.Stat()
	return err == nil && fi.Mode()&os.ModeCharDevice != 0
}

// flatten replaces every {"nodes": [...]} with its list, so a connection reads
// as an array in our JSON.
func flatten(v any) any {
	switch t := v.(type) {
	case map[string]any:
		if nodes, ok := t["nodes"]; ok && len(t) == 1 {
			return flatten(nodes)
		}
		for k, x := range t {
			t[k] = flatten(x)
		}
	case []any:
		for i, x := range t {
			t[i] = flatten(x)
		}
	}
	return v
}

// fieldsFlag is --fields as a list, checked against what the command returns.
func fieldsFlag(cmd *cobra.Command, known []string) ([]string, error) {
	raw, _ := cmd.Flags().GetString("fields")
	if raw == "" {
		return nil, nil
	}
	var fields []string
	for _, f := range strings.Split(raw, ",") {
		f = strings.TrimSpace(f)
		if !slices.Contains(known, f) {
			return nil, usageError{fmt.Errorf("--fields: no field %q; known: %s", f, strings.Join(known, ","))}
		}
		fields = append(fields, f)
	}
	return fields, nil
}

// pick keeps only fields of each item (all when fields is empty).
func pick(item map[string]any, fields []string) map[string]any {
	if len(fields) == 0 {
		return item
	}
	out := map[string]any{}
	for _, f := range fields {
		out[f] = item[f]
	}
	return out
}

// addFieldsFlag adds --fields and lists the known fields in the long help.
func addFieldsFlag(c *cobra.Command, known []string) {
	c.Flags().String("fields", "", "return only these comma-separated `FIELDS` (see Fields above)")
	if c.Long == "" {
		c.Long = c.Short + "."
	}
	c.Long += "\n\n" + wrap("Fields: "+strings.Join(known, ", "), 80)
}

// wrap breaks text into lines of at most width columns, between words.
func wrap(text string, width int) string {
	var lines []string
	line := ""
	for _, w := range strings.Fields(text) {
		if line != "" && len(line)+1+len(w) > width {
			lines = append(lines, line)
			line = ""
		}
		if line != "" {
			line += " "
		}
		line += w
	}
	return strings.Join(append(lines, line), "\n")
}

// page is one connection page as Linear returns it.
type page struct {
	Nodes    []map[string]any `json:"nodes"`
	PageInfo struct {
		HasNextPage bool   `json:"hasNextPage"`
		EndCursor   string `json:"endCursor"`
	} `json:"pageInfo"`
}

// collect pages through a connection until limit items (0: all). fetch gets
// one page of at most first items after a cursor.
func collect(limit int, cursor string, fetch func(first int, after string) (page, error)) (items []map[string]any, next string, err error) {
	items = []map[string]any{}
	for {
		first := pageSize
		if limit > 0 && limit-len(items) < first {
			first = limit - len(items)
		}
		p, err := fetch(first, cursor)
		if err != nil {
			return nil, "", err
		}
		items = append(items, p.Nodes...)
		if !p.PageInfo.HasNextPage {
			return items, "", nil
		}
		cursor = p.PageInfo.EndCursor
		if limit > 0 && len(items) >= limit {
			return items, cursor, nil
		}
	}
}

// listDoc is the list shape (conventions: JSON output). Linear connections
// have no count, so total is null; truncated says whether more remain, and
// nextCursor is where --cursor continues.
func listDoc(items []map[string]any, next string, fields []string) map[string]any {
	out := make([]map[string]any, len(items))
	for i, it := range items {
		out[i] = pick(flatten(it).(map[string]any), fields)
	}
	var cursor any
	if next != "" {
		cursor = next
		fmt.Fprintf(stderr, "%d shown, more remain; --cursor %s for the next page, --limit 0 for all\n", len(items), next)
	}
	return map[string]any{"items": out, "total": nil, "shown": len(out), "truncated": next != "", "nextCursor": cursor}
}

// str is a nested string field of a decoded item ("state.name"), "" if absent.
func str(item map[string]any, path string) string {
	var v any = item
	for _, k := range strings.Split(path, ".") {
		m, ok := v.(map[string]any)
		if !ok {
			return ""
		}
		v = m[k]
	}
	switch t := v.(type) {
	case string:
		return t
	case float64:
		return fmt.Sprint(t)
	case bool:
		return fmt.Sprint(t)
	}
	return ""
}

// rows prints items one per line, tab-separated, sanitized for a terminal.
func rows(cmd *cobra.Command, items []map[string]any, cols ...string) {
	for _, it := range items {
		vals := make([]string, len(cols))
		for i, c := range cols {
			vals[i] = printable(str(it, c))
			if vals[i] == "" {
				vals[i] = "-"
			}
		}
		fmt.Fprintln(cmd.OutOrStdout(), strings.Join(vals, "\t"))
	}
}

// limitFlag adds --limit with a default.
func limitFlag(c *cobra.Command, def int) *wholeNumber {
	n := &wholeNumber{n: def}
	c.Flags().Var(n, "limit", "show at most `N` items; 0 for all")
	return n
}

func cursorFlag(c *cobra.Command) {
	c.Flags().String("cursor", "", "continue after `CURSOR` (nextCursor from the previous page)")
}

func cursorValue(cmd *cobra.Command) (string, error) {
	c, _ := cmd.Flags().GetString("cursor")
	if c == "" {
		return "", nil
	}
	return plain("--cursor", c)
}
