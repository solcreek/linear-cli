package main

import (
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"testing"
)

// TestNoWorkspaceConventions keeps linearctl generic (docs/design.md, "What
// belongs in linearctl"): the source names no workspace and adds no hidden
// markers to text it sends. Extend the list when a convention tries to creep in.
func TestNoWorkspaceConventions(t *testing.T) {
	banned := regexp.MustCompile(`(?i)<!--|agent-context|\bkaik\b|manekineko|teachify`)
	files, _ := filepath.Glob("*.go")
	for _, f := range files {
		if strings.HasSuffix(f, "_test.go") {
			continue
		}
		data, err := os.ReadFile(f)
		must(t, err)
		for i, line := range strings.Split(string(data), "\n") {
			if m := banned.FindString(line); m != "" {
				t.Errorf("%s:%d: %q is a workspace convention, not Linear's API", f, i+1, m)
			}
		}
	}
}
