package main

import (
	"os"
	"testing"

	"github.com/vektah/gqlparser/v2"
	"github.com/vektah/gqlparser/v2/ast"
)

// TestDocumentsMatchSchema validates every document linearctl sends against
// Linear's published schema (graphql/schema.graphql).
func TestDocumentsMatchSchema(t *testing.T) {
	data, err := os.ReadFile("../../graphql/schema.graphql")
	if err != nil {
		t.Fatal(err)
	}
	schema, gerr := gqlparser.LoadSchema(&ast.Source{Name: "schema.graphql", Input: string(data)})
	if gerr != nil {
		t.Fatal(gerr)
	}
	for name, doc := range documents() {
		if got := opName(doc); got != name {
			t.Errorf("document %s is named %q", name, got)
		}
		if _, errs := gqlparser.LoadQuery(schema, doc); errs != nil {
			t.Errorf("%s: %v", name, errs)
		}
	}
}
