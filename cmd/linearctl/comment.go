package main

import (
	"errors"
	"fmt"
	"strings"

	"github.com/spf13/cobra"
)

var commentFields = []string{"id", "body", "createdAt", "url", "user", "parent"}

func addComment(root *cobra.Command) {
	comment := &cobra.Command{
		Use:   "comment",
		Short: "List and create comments on an issue",
		Args:  usageArgs(cobra.NoArgs),
		RunE:  func(cmd *cobra.Command, _ []string) error { return cmd.Help() },
	}

	list := &cobra.Command{
		Use:     "list ISSUE",
		Short:   "List an issue's comments, oldest first",
		Example: "  linearctl comment list ENG-123 --json\n  linearctl comment list ENG-123 --limit 0 --fields body,user --json",
		Args:    usageArgs(cobra.ExactArgs(1)),
		RunE: func(cmd *cobra.Command, args []string) error {
			ref, err := issueRef("ISSUE", args[0])
			if err != nil {
				return err
			}
			fields, err := fieldsFlag(cmd, commentFields)
			if err != nil {
				return err
			}
			cursor, err := cursorValue(cmd)
			if err != nil {
				return err
			}
			cl, err := newClient()
			if err != nil {
				return err
			}
			limit := cmd.Flags().Lookup("limit").Value.(*wholeNumber).n
			items, next, err := collect(limit, cursor, func(first int, after string) (page, error) {
				var out struct {
					Issue struct {
						Comments page `json:"comments"`
					} `json:"issue"`
				}
				err := cl.query(qCommentList, map[string]any{"id": ref, "first": first, "after": optional(after)}, &out)
				return out.Issue.Comments, notFound(err, "issue "+ref)
			})
			if err != nil {
				return err
			}
			doc := listDoc(items, next, fields)
			if asJSON {
				return printJSON(doc)
			}
			for _, it := range items {
				fmt.Fprintf(cmd.OutOrStdout(), "— %s, %s\n%s\n\n", printable(str(it, "user.name")), str(it, "createdAt"), printableText(str(it, "body")))
			}
			return nil
		},
	}
	limitFlag(list, 50)
	cursorFlag(list)
	addFieldsFlag(list, commentFields)

	create := &cobra.Command{
		Use:   "create ISSUE",
		Short: "Add a comment to an issue",
		Long: "Add a Markdown comment to an issue.\n\n" +
			"--yes requires --id, a UUID you choose, so a rerun can never post twice: if a\n" +
			"comment with that id exists, it is returned with changed: false.\n" +
			"--dry-run without --id generates one and prints it as id; pass it to --yes.\n" +
			"--parent replies in a comment's thread.\n" +
			"Changes things: pass --dry-run to preview, --yes to apply.",
		Example: "  linearctl comment create ENG-123 --body \"Fixed in #42\" --dry-run --json\n" +
			"  git log -1 --format=%B | linearctl comment create ENG-123 --body-file - --id 5f1e… --yes",
		Args: usageArgs(cobra.ExactArgs(1)),
		RunE: func(cmd *cobra.Command, args []string) error {
			dryRun, _ := change(cmd)
			ref, err := issueRef("ISSUE", args[0])
			if err != nil {
				return err
			}
			id, _ := cmd.Flags().GetString("id")
			switch {
			case id == "" && !dryRun:
				return usageError{errors.New("--yes needs --id UUID so a rerun cannot post twice; run --dry-run for one")}
			case id == "":
				id = newID()
			case !uuidPattern.MatchString(id):
				return usageError{fmt.Errorf("--id must be a UUID; got %q", id)}
			}
			id = strings.ToLower(id)
			body, set, err := text(cmd, "body")
			if err != nil {
				return err
			}
			if !set || strings.TrimSpace(body) == "" {
				return usageError{errors.New("pass --body TEXT or --body-file PATH (- for stdin), not empty")}
			}
			parent, _ := cmd.Flags().GetString("parent")
			if parent != "" && !uuidPattern.MatchString(parent) {
				return usageError{fmt.Errorf("--parent must be a comment id (a UUID, as comment list shows); got %q", parent)}
			}
			cl, err := newClient()
			if err != nil {
				return err
			}
			var r struct {
				Issue struct {
					ID         string `json:"id"`
					Identifier string `json:"identifier"`
				} `json:"issue"`
				Comments page `json:"comments"`
			}
			if err := cl.query(qCommentResolve, map[string]any{"issue": ref, "id": id}, &r); err != nil {
				return notFound(err, "issue "+ref)
			}
			if len(r.Comments.Nodes) > 0 {
				it := flatten(r.Comments.Nodes[0]).(map[string]any)
				return output(cmd, map[string]any{"changed": false, "dryRun": dryRun, "id": id, "comment": it},
					"exists "+str(it, "url"))
			}
			input := map[string]any{"id": id, "issueId": r.Issue.ID, "body": body}
			if parent != "" {
				input["parentId"] = strings.ToLower(parent)
			}
			if dryRun {
				return output(cmd, map[string]any{"changed": true, "dryRun": true, "id": id, "input": input},
					fmt.Sprintf("would comment on %s\nid: %s (pass --id %s --yes)", r.Issue.Identifier, id, id))
			}
			var out struct {
				CommentCreate struct {
					Comment map[string]any `json:"comment"`
				} `json:"commentCreate"`
			}
			if err := cl.query(qCommentCreate, map[string]any{"input": input}, &out); err != nil {
				return err
			}
			it := flatten(out.CommentCreate.Comment).(map[string]any)
			return output(cmd, map[string]any{"changed": true, "dryRun": false, "id": id, "comment": it}, "commented "+str(it, "url"))
		},
	}
	create.Flags().String("body", "", "the comment, Markdown `TEXT`")
	create.Flags().String("body-file", "", "read the comment from `PATH` (- for stdin)")
	create.Flags().String("id", "", "the new comment's `UUID`; required with --yes")
	create.Flags().String("parent", "", "reply in the thread of comment `ID`")
	mutating(create)

	comment.AddCommand(list, create, commentUpdate(), commentDelete())
	root.AddCommand(comment)
}

// commentID is a COMMENT argument: a comment UUID, as comment list shows it.
func commentID(s string) (string, error) {
	if _, err := plain("COMMENT", s); err != nil {
		return "", err
	}
	if !uuidPattern.MatchString(s) {
		return "", usageError{fmt.Errorf("COMMENT must be a comment id (a UUID, as comment list shows); got %q", s)}
	}
	return strings.ToLower(s), nil
}

func commentUpdate() *cobra.Command {
	c := &cobra.Command{
		Use:   "update COMMENT",
		Short: "Replace a comment's text",
		Long: "Replace a comment's Markdown text. The same text again is changed: false.\n" +
			"Changes things: pass --dry-run to preview, --yes to apply.",
		Example: "  linearctl comment update 5f1e… --body-file note.md --dry-run --json\n" +
			"  linearctl comment update 5f1e… --body \"Fixed in #43\" --yes",
		Args: usageArgs(cobra.ExactArgs(1)),
		RunE: func(cmd *cobra.Command, args []string) error {
			dryRun, _ := change(cmd)
			id, err := commentID(args[0])
			if err != nil {
				return err
			}
			body, set, err := text(cmd, "body")
			if err != nil {
				return err
			}
			if !set || strings.TrimSpace(body) == "" {
				return usageError{errors.New("pass --body TEXT or --body-file PATH (- for stdin), not empty")}
			}
			cl, err := newClient()
			if err != nil {
				return err
			}
			var r struct {
				Comment map[string]any `json:"comment"`
			}
			if err := cl.query(qComment, map[string]any{"id": id}, &r); err != nil {
				return notFound(err, "comment "+id)
			}
			changed := str(r.Comment, "body") != body
			doc := map[string]any{"changed": changed, "dryRun": dryRun, "id": id}
			switch {
			case !changed:
				doc["comment"] = flatten(r.Comment)
				return output(cmd, doc, "unchanged "+str(r.Comment, "url"))
			case dryRun:
				doc["input"] = map[string]any{"body": body}
				return output(cmd, doc, "would update "+str(r.Comment, "url"))
			}
			var out struct {
				CommentUpdate struct {
					Comment map[string]any `json:"comment"`
				} `json:"commentUpdate"`
			}
			if err := cl.query(qCommentUpdate, map[string]any{"id": id, "input": map[string]any{"body": body}}, &out); err != nil {
				return err
			}
			doc["comment"] = flatten(out.CommentUpdate.Comment)
			return output(cmd, doc, "updated "+str(out.CommentUpdate.Comment, "url"))
		},
	}
	c.Flags().String("body", "", "the new text, Markdown `TEXT`")
	c.Flags().String("body-file", "", "read the new text from `PATH` (- for stdin)")
	mutating(c)
	return c
}

func commentDelete() *cobra.Command {
	c := &cobra.Command{
		Use:   "delete COMMENT",
		Short: "Delete a comment",
		Long: "Delete a comment. A comment that is not there (already deleted, or never was)\n" +
			"is changed: false with found: false.\n" +
			"Changes things: pass --dry-run to preview, --yes to apply.",
		Example: "  linearctl comment delete 5f1e… --dry-run\n  linearctl comment delete 5f1e… --yes --json",
		Args:    usageArgs(cobra.ExactArgs(1)),
		RunE: func(cmd *cobra.Command, args []string) error {
			dryRun, _ := change(cmd)
			id, err := commentID(args[0])
			if err != nil {
				return err
			}
			cl, err := newClient()
			if err != nil {
				return err
			}
			var r struct {
				Comment map[string]any `json:"comment"`
			}
			err = cl.query(qComment, map[string]any{"id": id}, &r)
			var le linearError
			if errors.As(err, &le) && le.notFound {
				return output(cmd, map[string]any{"changed": false, "dryRun": dryRun, "id": id, "found": false}, "no comment "+id)
			}
			if err != nil {
				return err
			}
			if !dryRun {
				if err := cl.query(qCommentDelete, map[string]any{"id": id}, nil); err != nil {
					return err
				}
			}
			verb := map[bool]string{true: "would delete ", false: "deleted "}[dryRun]
			return output(cmd, map[string]any{"changed": true, "dryRun": dryRun, "id": id, "found": true, "url": str(r.Comment, "url")},
				verb+str(r.Comment, "url"))
		},
	}
	mutating(c)
	return c
}
