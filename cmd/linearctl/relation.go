package main

import (
	"fmt"
	"slices"
	"strings"

	"github.com/spf13/cobra"
)

// relationTypes are TYPE as the commands take it. Linear stores blocks,
// duplicate, related and similar; blocked-by is blocks with the issues
// swapped. related and similar hold in both directions.
var relationTypes = []string{"blocks", "blocked-by", "duplicate", "related", "similar"}

const relationHelp = "TYPE is one of:\n" +
	"  blocks      ISSUE blocks OTHER\n" +
	"  blocked-by  OTHER blocks ISSUE\n" +
	"  duplicate   ISSUE is a duplicate of OTHER\n" +
	"  related     ISSUE and OTHER are related (either direction)\n" +
	"  similar     ISSUE and OTHER are similar (either direction)"

func addRelation(root *cobra.Command) {
	relation := &cobra.Command{
		Use:   "relation",
		Short: "Create and delete relations between issues (blocks, duplicate, related)",
		Args:  usageArgs(cobra.NoArgs),
		RunE:  func(cmd *cobra.Command, _ []string) error { return cmd.Help() },
	}
	relation.AddCommand(relationCommand(true), relationCommand(false))
	root.AddCommand(relation)
}

// relationArgs checks ISSUE TYPE OTHER, before anything is sent.
func relationArgs(args []string) (from, kind, to string, err error) {
	if from, err = issueRef("ISSUE", args[0]); err != nil {
		return
	}
	if kind = args[1]; !slices.Contains(relationTypes, kind) {
		err = usageError{fmt.Errorf("TYPE must be one of %s; got %q", strings.Join(relationTypes, ", "), kind)}
		return
	}
	if to, err = issueRef("OTHER", args[2]); err != nil {
		return
	}
	if from == to {
		err = usageError{fmt.Errorf("an issue cannot relate to itself: %s", from)}
	}
	return
}

// relationEnds are the two issues as Linear's relation has them, and its type.
type relationEnds struct {
	issue, related *named
	kind           string
}

type relatedIssue struct {
	ID         string `json:"id"`
	Identifier string `json:"identifier"`
	Relations  struct {
		Nodes []struct {
			ID           string `json:"id"`
			Type         string `json:"type"`
			RelatedIssue named  `json:"relatedIssue"`
		} `json:"nodes"`
	} `json:"relations"`
	InverseRelations struct {
		Nodes []struct {
			ID    string `json:"id"`
			Type  string `json:"type"`
			Issue named  `json:"issue"`
		} `json:"nodes"`
	} `json:"inverseRelations"`
}

// existing is the id of the relation ends describes, "" if there is none.
// a is one of its two issues, with the relations Linear stores from it
// (a, related) and to it (issue, a). related and similar also match reversed.
func existing(a relatedIssue, ends relationEnds) string {
	symmetric := ends.kind == "related" || ends.kind == "similar"
	match := func(kind, issue, related string) bool {
		return kind == ends.kind && ((issue == ends.issue.ID && related == ends.related.ID) ||
			(symmetric && issue == ends.related.ID && related == ends.issue.ID))
	}
	for _, r := range a.Relations.Nodes {
		if match(r.Type, a.ID, r.RelatedIssue.ID) {
			return r.ID
		}
	}
	for _, r := range a.InverseRelations.Nodes {
		if match(r.Type, r.Issue.ID, a.ID) {
			return r.ID
		}
	}
	return ""
}

func relationCommand(create bool) *cobra.Command {
	verb := map[bool]string{true: "create", false: "delete"}[create]
	c := &cobra.Command{
		Use:   verb + " ISSUE TYPE OTHER",
		Short: map[bool]string{true: "Relate two issues", false: "Remove a relation between two issues"}[create],
		Long: map[bool]string{
			true:  "Relate two issues. A relation that already exists is changed: false.\n",
			false: "Remove a relation between two issues. One that is not there is changed: false.\n",
		}[create] + relationHelp + "\nChanges things: pass --dry-run to preview, --yes to apply.",
		Example: fmt.Sprintf("  linearctl relation %[1]s ENG-12 blocks ENG-15 --dry-run\n"+
			"  linearctl relation %[1]s ENG-12 duplicate ENG-9 --yes --json", verb),
		Args: usageArgs(cobra.ExactArgs(3)),
		RunE: func(cmd *cobra.Command, args []string) error {
			dryRun, _ := change(cmd)
			from, kind, to, err := relationArgs(args)
			if err != nil {
				return err
			}
			cl, err := newClient()
			if err != nil {
				return err
			}
			var r struct {
				A struct{ Nodes []relatedIssue } `json:"a"`
				B struct{ Nodes []named }        `json:"b"`
			}
			if err := cl.query(qRelationResolve, map[string]any{"a": issueFilter(from), "b": issueFilter(to)}, &r); err != nil {
				return err
			}
			switch {
			case len(r.A.Nodes) == 0:
				return fmt.Errorf("not found: issue %s (or this API key cannot see it)", from)
			case len(r.B.Nodes) == 0:
				return fmt.Errorf("not found: issue %s (or this API key cannot see it)", to)
			}
			a := r.A.Nodes[0]
			ends := relationEnds{&named{ID: a.ID, Identifier: a.Identifier}, &r.B.Nodes[0], kind}
			if kind == "blocked-by" {
				ends = relationEnds{ends.related, ends.issue, "blocks"}
			}
			id := existing(a, ends)
			changed := (id == "") == create
			doc := map[string]any{"changed": changed, "dryRun": dryRun,
				"relation": map[string]any{"id": optional(id), "type": ends.kind, "issue": ends.issue.Identifier, "relatedIssue": ends.related.Identifier}}
			text := fmt.Sprintf("%s %s %s", ends.issue.Identifier, ends.kind, ends.related.Identifier)
			if !changed {
				return output(cmd, doc, map[bool]string{true: "exists: ", false: "no relation: "}[create]+text)
			}
			if dryRun {
				return output(cmd, doc, "would "+verb+": "+text)
			}
			if create {
				input := map[string]any{"issueId": ends.issue.ID, "relatedIssueId": ends.related.ID, "type": ends.kind}
				var out struct {
					IssueRelationCreate struct {
						IssueRelation struct {
							ID string `json:"id"`
						} `json:"issueRelation"`
					} `json:"issueRelationCreate"`
				}
				if err := cl.query(qRelationCreate, map[string]any{"input": input}, &out); err != nil {
					return err
				}
				doc["relation"].(map[string]any)["id"] = out.IssueRelationCreate.IssueRelation.ID
			} else if err := cl.query(qRelationDelete, map[string]any{"id": id}, nil); err != nil {
				return err
			}
			return output(cmd, doc, verb+"d: "+text)
		},
	}
	mutating(c)
	return c
}
