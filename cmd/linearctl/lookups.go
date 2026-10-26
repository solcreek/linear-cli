package main

import (
	"fmt"
	"slices"
	"strings"

	"github.com/spf13/cobra"
)

// lookup is one "NOUN list" command: what an agent or a picker needs to name
// things (states, labels, users) in other commands.
type lookup struct {
	noun, short, long, example string
	doc, conn                  string
	fields                     []string
	cols                       []string // the human columns
	team                       bool     // takes --team
	// filter is the connection's filter for the command's flags; nil for none.
	filter func(cmd *cobra.Command, team string) (map[string]any, error)
	// sort orders the items, for lists Linear does not order usefully.
	sort func(a, b map[string]any) int
}

func addLookups(root *cobra.Command) {
	for _, l := range []lookup{
		{
			noun: "team", short: "List teams", conn: "teams", doc: qTeams,
			example: "  linearctl team list --json",
			fields:  []string{"id", "key", "name", "description", "private"},
			cols:    []string{"key", "name"},
		},
		{
			noun: "state", short: "List workflow states, by team and position", conn: "workflowStates", doc: qStates,
			long:    "List workflow states, by team and position: the names issue create and update\ntake for --state.",
			example: "  linearctl state list --team ENG --json",
			fields:  []string{"id", "name", "type", "position", "color", "team"},
			cols:    []string{"team.key", "name", "type"},
			team:    true,
			filter: func(_ *cobra.Command, team string) (map[string]any, error) {
				if team == "" {
					return nil, nil
				}
				return map[string]any{"team": map[string]any{"key": map[string]any{"eqIgnoreCase": team}}}, nil
			},
			sort: func(a, b map[string]any) int {
				if c := strings.Compare(str(a, "team.key"), str(b, "team.key")); c != 0 {
					return c
				}
				pa, _ := a["position"].(float64)
				pb, _ := b["position"].(float64)
				return cmpFloat(pa, pb)
			},
		},
		{
			noun: "label", short: "List issue labels", conn: "issueLabels", doc: qLabels,
			long: "List issue labels. With --team, the team's labels and the workspace's (no team),\n" +
				"which are the ones its issues can take.",
			example: "  linearctl label list --team ENG --json",
			fields:  []string{"id", "name", "color", "isGroup", "team", "parent"},
			cols:    []string{"name", "team.key", "parent.name"},
			team:    true,
			filter: func(_ *cobra.Command, team string) (map[string]any, error) {
				if team == "" {
					return nil, nil
				}
				return map[string]any{"or": []any{
					map[string]any{"team": map[string]any{"key": map[string]any{"eqIgnoreCase": team}}},
					map[string]any{"team": map[string]any{"null": true}},
				}}, nil
			},
			sort: func(a, b map[string]any) int { return strings.Compare(str(a, "name"), str(b, "name")) },
		},
		{
			noun: "user", short: "List users in the workspace", conn: "users", doc: qUsers,
			example: "  linearctl user list --active --json",
			fields:  []string{"id", "name", "displayName", "email", "active", "admin", "guest"},
			cols:    []string{"email", "name"},
			filter: func(cmd *cobra.Command, _ string) (map[string]any, error) {
				if active, _ := cmd.Flags().GetBool("active"); active {
					return map[string]any{"active": map[string]any{"eq": true}}, nil
				}
				return nil, nil
			},
		},
		{
			noun: "project", short: "List projects", conn: "projects", doc: qProjects,
			example: "  linearctl project list --team ENG --status started --json",
			fields:  []string{"id", "name", "url", "targetDate", "status", "lead"},
			cols:    []string{"name", "status.name", "lead.name"},
			team:    true,
			filter: func(cmd *cobra.Command, team string) (map[string]any, error) {
				f := map[string]any{}
				if team != "" {
					f["accessibleTeams"] = map[string]any{"some": map[string]any{"key": map[string]any{"eqIgnoreCase": team}}}
				}
				s, err := values(cmd, "status")
				if err != nil {
					return nil, err
				}
				if len(s) > 0 {
					for _, t := range s {
						if !slices.Contains(projectStatuses, t) {
							return nil, usageError{fmt.Errorf("--status must be one of %s; got %q", strings.Join(projectStatuses, ", "), t)}
						}
					}
					f["status"] = map[string]any{"type": map[string]any{"in": s}}
				}
				if len(f) == 0 {
					return nil, nil
				}
				return f, nil
			},
		},
		{
			noun: "cycle", short: "List cycles", conn: "cycles", doc: qCycles,
			example: "  linearctl cycle list --team ENG --json",
			fields:  []string{"id", "number", "name", "startsAt", "endsAt", "isActive", "isNext", "team"},
			cols:    []string{"team.key", "number", "startsAt", "endsAt"},
			team:    true,
			filter: func(_ *cobra.Command, team string) (map[string]any, error) {
				if team == "" {
					return nil, nil
				}
				return map[string]any{"team": map[string]any{"key": map[string]any{"eqIgnoreCase": team}}}, nil
			},
		},
	} {
		root.AddCommand(l.command())
	}
}

var projectStatuses = []string{"backlog", "planned", "started", "paused", "completed", "canceled"}

func cmpFloat(a, b float64) int {
	switch {
	case a < b:
		return -1
	case a > b:
		return 1
	}
	return 0
}

func (l lookup) command() *cobra.Command {
	parent := &cobra.Command{
		Use:   l.noun,
		Short: l.short,
		Args:  usageArgs(cobra.NoArgs),
		RunE:  func(cmd *cobra.Command, _ []string) error { return cmd.Help() },
	}
	list := &cobra.Command{
		Use:     "list",
		Short:   l.short,
		Long:    l.long,
		Example: l.example,
		Args:    usageArgs(cobra.NoArgs),
		RunE: func(cmd *cobra.Command, _ []string) error {
			fields, err := fieldsFlag(cmd, l.fields)
			if err != nil {
				return err
			}
			cursor, err := cursorValue(cmd)
			if err != nil {
				return err
			}
			team := ""
			if l.team {
				if team, err = teamFlag(cmd); err != nil {
					return err
				}
			}
			var filter map[string]any
			if l.filter != nil {
				if filter, err = l.filter(cmd, team); err != nil {
					return err
				}
			}
			cl, err := newClient()
			if err != nil {
				return err
			}
			limit := cmd.Flags().Lookup("limit").Value.(*wholeNumber).n
			items, next, err := collect(limit, cursor, func(first int, after string) (page, error) {
				vars := map[string]any{"first": first, "after": optional(after)}
				if filter != nil {
					vars["filter"] = filter
				}
				var out map[string]page
				err := cl.query(l.doc, vars, &out)
				return out[l.conn], err
			})
			if err != nil {
				return err
			}
			if l.sort != nil {
				slices.SortStableFunc(items, l.sort)
			}
			doc := listDoc(items, next, fields)
			if asJSON {
				return printJSON(doc)
			}
			rows(cmd, items, l.cols...)
			return nil
		},
	}
	if l.team {
		list.Flags().String("team", "", "only team `KEY` (falls back to LINEARCTL_TEAM, then default-team)")
	}
	switch l.noun {
	case "user":
		list.Flags().Bool("active", false, "only active users")
	case "project":
		list.Flags().StringArray("status", nil, "only projects whose status is `TYPE` (repeatable):\n"+strings.Join(projectStatuses, ", "))
	}
	limitFlag(list, 50)
	cursorFlag(list)
	addFieldsFlag(list, l.fields)
	parent.AddCommand(list)
	return parent
}
