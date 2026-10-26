package main

import (
	"errors"
	"fmt"
	"net/url"
	"regexp"
	"strings"

	"github.com/spf13/cobra"
)

var slugPattern = regexp.MustCompile(`^[0-9a-f]{12}$`)

var projectFields = []string{"id", "name", "slugId", "url", "description", "content", "status", "lead", "startDate",
	"targetDate", "progress", "teams", "projectMilestones", "issues"}

// addProject adds show and update to the project command that addLookups
// made for project list.
func addProject(root *cobra.Command) {
	for _, c := range root.Commands() {
		if c.Name() == "project" {
			c.AddCommand(projectShow(), projectUpdate())
			c.Short = "List, show and update projects"
		}
	}
}

// projectFilter finds a PROJECT given as a linear.app project URL, its
// slugId (the hex at the end of that URL), a UUID, or its exact name.
func projectFilter(s string) (map[string]any, error) {
	if _, err := plain("PROJECT", s); err != nil {
		return nil, err
	}
	if strings.HasPrefix(s, "https://") {
		u, err := url.Parse(s)
		parts := []string{}
		if err == nil {
			parts = strings.Split(strings.Trim(u.Path, "/"), "/")
		}
		if err != nil || u.Host != "linear.app" || len(parts) < 3 || parts[1] != "project" {
			return nil, usageError{fmt.Errorf("PROJECT: not a linear.app project URL: %s", s)}
		}
		slug := parts[2][strings.LastIndex(parts[2], "-")+1:]
		if !slugPattern.MatchString(slug) {
			return nil, usageError{fmt.Errorf("PROJECT: no project id at the end of %s", s)}
		}
		return map[string]any{"slugId": map[string]any{"eq": slug}}, nil
	}
	switch {
	case uuidPattern.MatchString(s):
		return map[string]any{"id": map[string]any{"eq": strings.ToLower(s)}}, nil
	case slugPattern.MatchString(s):
		return map[string]any{"slugId": map[string]any{"eq": s}}, nil
	}
	return map[string]any{"name": map[string]any{"eqIgnoreCase": s}}, nil
}

// findProject is the one project PROJECT names: none is not found, and a
// name two projects share is an error listing them.
func findProject(cl *client, ref string, filter map[string]any, issues int) (map[string]any, error) {
	var out struct {
		Projects struct {
			Nodes []map[string]any `json:"nodes"`
		} `json:"projects"`
	}
	vars := map[string]any{"filter": filter, "issues": issues, "withIssues": issues > 0}
	if err := cl.query(qProjectFind, vars, &out); err != nil {
		return nil, err
	}
	switch n := out.Projects.Nodes; len(n) {
	case 0:
		return nil, fmt.Errorf("not found: project %s (or this API key cannot see it)", ref)
	case 1:
		return n[0], nil
	default:
		return nil, fmt.Errorf("more than one project is named %q (%s, %s); pass its URL or slugId", ref, str(n[0], "url"), str(n[1], "url"))
	}
}

func projectShow() *cobra.Command {
	c := &cobra.Command{
		Use:   "show PROJECT",
		Short: "Show a project: its overview, milestones and issues",
		Long: "Show a project: its description, overview (content), status, milestones and issues.\n" +
			"PROJECT is a linear.app project URL, its slugId (the hex the URL ends with), a UUID,\n" +
			"or its exact name.",
		Example: "  linearctl project show https://linear.app/acme/project/q4-launch-0527ca4c9128\n" +
			"  linearctl project show \"Q4 launch\" --issues 100 --fields name,content,issues --json",
		Args: usageArgs(cobra.ExactArgs(1)),
		RunE: func(cmd *cobra.Command, args []string) error {
			filter, err := projectFilter(args[0])
			if err != nil {
				return err
			}
			fields, err := fieldsFlag(cmd, projectFields)
			if err != nil {
				return err
			}
			issues := cmd.Flags().Lookup("issues").Value.(*wholeNumber).n
			if issues > 250 {
				return usageError{errors.New("--issues is at most 250; page with 'linearctl issue list --project'")}
			}
			cl, err := newClient()
			if err != nil {
				return err
			}
			p, err := findProject(cl, args[0], filter, issues)
			if err != nil {
				return err
			}
			p = flatten(p).(map[string]any)
			if asJSON {
				return printJSON(map[string]any{"project": pick(p, fields)})
			}
			w := cmd.OutOrStdout()
			fmt.Fprintf(w, "%s  (%s)\n%s\n", printable(str(p, "name")), printable(str(p, "status.name")), str(p, "url"))
			if d := str(p, "description"); d != "" {
				fmt.Fprintf(w, "\n%s\n", printable(d))
			}
			if body := str(p, "content"); body != "" {
				fmt.Fprintf(w, "\n%s\n", printableText(body))
			}
			if is, _ := p["issues"].([]any); len(is) > 0 {
				fmt.Fprintf(w, "\n%d issues\n", len(is))
				items := []map[string]any{}
				for _, x := range is {
					items = append(items, x.(map[string]any))
				}
				rows(cmd, items, "identifier", "state.name", "assignee.name", "title")
			}
			return nil
		},
	}
	c.Flags().Var(&wholeNumber{n: 50}, "issues", "include the first `N` issues; 0 for none")
	addFieldsFlag(c, projectFields)
	return c
}

func projectUpdate() *cobra.Command {
	c := &cobra.Command{
		Use:   "update PROJECT",
		Short: "Change a project's name, description or overview",
		Long: "Change a project's name, description (its one-line summary) or content (the\n" +
			"overview document; the new text replaces all of it). Only fields that differ are\n" +
			"sent; when none do, nothing is sent and the result is changed: false.\n" +
			"PROJECT is a URL, slugId, UUID or exact name, as for project show.\n" +
			"Changes things: pass --dry-run to preview, --yes to apply.",
		Example: "  linearctl project update \"Q4 launch\" --content-file overview.md --dry-run --json\n" +
			"  linearctl project update 0527ca4c9128 --description \"Ship the new login\" --yes",
		Args: usageArgs(cobra.ExactArgs(1)),
		RunE: func(cmd *cobra.Command, args []string) error {
			dryRun, _ := change(cmd)
			filter, err := projectFilter(args[0])
			if err != nil {
				return err
			}
			input := map[string]any{}
			if cmd.Flags().Changed("name") {
				name, _ := cmd.Flags().GetString("name")
				if _, err := plain("--name", name); err != nil {
					return err
				}
				input["name"] = name
			}
			for _, f := range []string{"description", "content"} {
				v, set, err := text(cmd, f)
				if err != nil {
					return err
				}
				if set {
					input[f] = v
				}
			}
			if len(input) == 0 {
				return usageError{errors.New("pass --name, --description, or --content / --content-file")}
			}
			cl, err := newClient()
			if err != nil {
				return err
			}
			p, err := findProject(cl, args[0], filter, 0)
			if err != nil {
				return err
			}
			for k, v := range input {
				if str(p, k) == v {
					delete(input, k)
				}
			}
			changed := len(input) > 0
			doc := map[string]any{"changed": changed, "dryRun": dryRun, "id": p["id"], "url": p["url"], "input": input}
			name := printable(str(p, "name"))
			switch {
			case !changed:
				return output(cmd, doc, "unchanged "+name)
			case dryRun:
				return output(cmd, doc, "would update "+name)
			}
			if err := cl.query(qProjectUpdate, map[string]any{"id": p["id"], "input": input}, nil); err != nil {
				return err
			}
			return output(cmd, doc, "updated "+name)
		},
	}
	c.Flags().String("name", "", "the new name, `TEXT`")
	c.Flags().String("description", "", "the new one-line summary, `TEXT`")
	c.Flags().String("description-file", "", "read the summary from `PATH` (- for stdin)")
	c.Flags().String("content", "", "the new overview, Markdown `TEXT`; replaces all of it")
	c.Flags().String("content-file", "", "read the overview from `PATH` (- for stdin)")
	mutating(c)
	return c
}
