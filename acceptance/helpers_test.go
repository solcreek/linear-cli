package acceptance

import (
	"fmt"
	"regexp"
	"strings"
)

// The exit codes, as docs/design.md lists them.
const (
	exitOK       = 0
	exitFailed   = 1
	exitUsage    = 2
	exitPartial  = 3
	exitBusy     = 4
	exitAuth     = 5
	exitLimited  = 6
	exitConflict = 7
)

// pageSize is the page linearctl asks Linear for.
const pageSize = 250

var uuidPattern = regexp.MustCompile(`^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`)

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

// opName is a document's operation name, which the fake answers by.
func opName(doc string) string {
	f := strings.Fields(doc)
	if len(f) < 2 {
		return ""
	}
	name, _, _ := strings.Cut(f[1], "(")
	return name
}
