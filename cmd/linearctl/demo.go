package main

import (
	"crypto/sha1"
	_ "embed"
	"encoding/json"
	"fmt"
	"regexp"
	"slices"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/vektah/gqlparser/v2/ast"
	"github.com/vektah/gqlparser/v2/parser"
)

// Demo mode (--demo or LINEARCTL_DEMO=1) answers every command from a
// fictional workspace built into the binary, with no API key and no network.
// It swaps the transport, not the commands: each command sends its usual
// GraphQL document, and demoAnswer resolves it against the workspace, so
// demo output has the shapes of real output by construction. Writes are
// refused; --dry-run works, since it only reads.
//
// demo/workspace.json is written for people: issues name their state,
// labels and users, and times are relative to "anchor", which is moved to
// now. linear-tui drafts it; linearctl ships it.

//go:embed demo/workspace.json
var demoSeed []byte

var demo bool

const demoOrg = "northwind"

// demoID is a stable UUID for a name.
func demoID(kind, name string) string {
	h := sha1.Sum([]byte(kind + ":" + name))
	h[6] = h[6]&0x0f | 0x50
	h[8] = h[8]&0x3f | 0x80
	return fmt.Sprintf("%x-%x-%x-%x-%x", h[0:4], h[4:6], h[6:8], h[8:10], h[10:16])
}

var slugDrop = regexp.MustCompile(`[^a-z0-9]+`)

func slug(s string) string {
	s = strings.Trim(slugDrop.ReplaceAllString(strings.ToLower(s), "-"), "-")
	if len(s) > 60 {
		s = strings.TrimRight(s[:60], "-")
	}
	return s
}

var demoWorld = sync.OnceValue(buildDemo)

// world is the expanded workspace: root lists as Linear's queries name them.
type world struct {
	root   map[string][]any
	viewer map[string]any
	org    map[string]any
}

func buildDemo() *world {
	var seed struct {
		Anchor       string           `json:"anchor"`
		Organization map[string]any   `json:"organization"`
		Viewer       string           `json:"viewer"`
		Teams        []map[string]any `json:"teams"`
		Users        []map[string]any `json:"users"`
		States       []map[string]any `json:"states"`
		Labels       []map[string]any `json:"labels"`
		Projects     []map[string]any `json:"projects"`
		Cycles       []map[string]any `json:"cycles"`
		Issues       []map[string]any `json:"issues"`
	}
	if err := json.Unmarshal(demoSeed, &seed); err != nil {
		panic("demo/workspace.json: " + err.Error())
	}
	anchor, _ := time.Parse(time.RFC3339, seed.Anchor)
	now := time.Now().UTC()
	shift := now.Sub(anchor)
	at := func(v any) any {
		s, _ := v.(string)
		if t, err := time.Parse(time.RFC3339, s); err == nil {
			return t.Add(shift).UTC().Format("2006-01-02T15:04:05.000Z")
		}
		if t, err := time.Parse("2006-01-02", s); err == nil {
			return t.Add(shift).Format("2006-01-02")
		}
		return nil
	}
	w := &world{root: map[string][]any{}}
	w.org = map[string]any{"id": demoID("org", demoOrg), "name": seed.Organization["name"], "urlKey": seed.Organization["urlKey"]}
	urlKey, _ := seed.Organization["urlKey"].(string)
	users := map[string]map[string]any{}
	for _, u := range seed.Users {
		email := u["email"].(string)
		m := map[string]any{"__typename": "User", "id": demoID("user", email), "name": u["name"], "displayName": u["displayName"],
			"email": email, "active": true, "admin": u["admin"] == true, "guest": false, "app": false, "isMe": email == seed.Viewer}
		users[email] = m
		w.root["users"] = append(w.root["users"], m)
	}
	w.viewer = users[seed.Viewer]
	team := map[string]any{}
	for _, t := range seed.Teams {
		team = map[string]any{"id": demoID("team", t["key"].(string)), "key": t["key"], "name": t["name"],
			"description": t["description"], "private": t["private"] == true}
		w.root["teams"] = append(w.root["teams"], team)
	}
	states := map[string]map[string]any{}
	for i, s := range seed.States {
		m := map[string]any{"id": demoID("state", s["name"].(string)), "name": s["name"], "type": s["type"], "color": s["color"],
			"position": float64(i), "team": team}
		states[s["name"].(string)] = m
		w.root["workflowStates"] = append(w.root["workflowStates"], m)
	}
	labels := map[string]map[string]any{}
	for _, l := range seed.Labels {
		m := map[string]any{"id": demoID("label", l["name"].(string)), "name": l["name"], "color": l["color"],
			"isGroup": false, "team": nil, "parent": nil}
		labels[l["name"].(string)] = m
		w.root["issueLabels"] = append(w.root["issueLabels"], m)
	}
	cycles := map[float64]map[string]any{}
	var next map[string]any
	for _, c := range seed.Cycles {
		starts, ends := at(c["startsAt"]), at(c["endsAt"])
		s, _ := time.Parse(time.RFC3339, starts.(string))
		e, _ := time.Parse(time.RFC3339, ends.(string))
		m := map[string]any{"id": demoID("cycle", fmt.Sprint(c["number"])), "number": c["number"], "name": nil,
			"startsAt": starts, "endsAt": ends, "isActive": !now.Before(s) && now.Before(e), "isNext": false, "team": team}
		if now.Before(s) && (next == nil || starts.(string) < next["startsAt"].(string)) {
			next = m
		}
		cycles[c["number"].(float64)] = m
		w.root["cycles"] = append(w.root["cycles"], m)
	}
	if next != nil {
		next["isNext"] = true
	}
	team["states"] = append([]any{}, w.root["workflowStates"]...)
	team["labels"] = []any{} // the demo's labels belong to the workspace
	team["cycles"] = append([]any{}, w.root["cycles"]...)
	statusNames := map[string]string{"backlog": "Backlog", "planned": "Planned", "started": "In Progress",
		"paused": "Paused", "completed": "Completed", "canceled": "Canceled"}
	projects := map[string]map[string]any{}
	for _, p := range seed.Projects {
		name := p["name"].(string)
		slugID := demoID("project", name)[24:]
		kind := p["status"].(string)
		m := map[string]any{"id": demoID("project", name), "name": name, "slugId": slugID,
			"url":         fmt.Sprintf("https://linear.app/%s/project/%s-%s", urlKey, slug(name), slugID),
			"description": "", "content": "", "startDate": nil, "targetDate": at(p["targetDate"]), "progress": 0.0,
			"status": map[string]any{"id": demoID("status", kind), "name": statusNames[kind], "type": kind},
			"lead":   users[fmt.Sprint(p["lead"])], "teams": []any{team}, "projectMilestones": []any{}, "issues": []any{}}
		projects[name] = m
		w.root["projects"] = append(w.root["projects"], m)
	}
	priorityLabels := []string{"No priority", "Urgent", "High", "Medium", "Low"}
	issues := map[string]map[string]any{}
	for _, s := range seed.Issues {
		ident := s["identifier"].(string)
		number, _ := strconv.Atoi(ident[strings.LastIndex(ident, "-")+1:])
		prio := int(s["priority"].(float64))
		m := map[string]any{"__typename": "Issue", "id": demoID("issue", ident), "identifier": ident, "number": float64(number),
			"title": s["title"], "description": s["description"], "priority": float64(prio), "priorityLabel": priorityLabels[prio],
			"url":        fmt.Sprintf("https://linear.app/%s/issue/%s/%s", urlKey, ident, slug(s["title"].(string))),
			"branchName": fmt.Sprintf("%s/%s-%s", strings.SplitN(fmt.Sprint(s["assignee"]), "@", 2)[0], strings.ToLower(ident), slug(s["title"].(string))),
			"createdAt":  at(s["createdAt"]), "updatedAt": at(s["updatedAt"]), "startedAt": at(s["startedAt"]),
			"completedAt": at(s["completedAt"]), "canceledAt": nil, "archivedAt": nil, "trashed": false,
			"dueDate": at(s["dueDate"]), "estimate": s["estimate"], "state": states[s["state"].(string)], "team": team,
			"assignee": nilIfMissing(users, s["assignee"]), "creator": nilIfMissing(users, s["creator"]), "delegate": nil,
			"project": nilIfMissing(projects, s["project"]), "projectMilestone": nil, "parent": nil,
			"children": []any{}, "relations": []any{}, "inverseRelations": []any{}, "comments": []any{}, "attachments": []any{}}
		if c, ok := s["cycle"].(float64); ok {
			m["cycle"] = cycles[c]
		} else {
			m["cycle"] = nil
		}
		var ls []any
		for _, l := range s["labels"].([]any) {
			ls = append(ls, labels[l.(string)])
		}
		m["labels"] = append([]any{}, ls...)
		for _, a := range asList(s["attachments"]) {
			a := a.(map[string]any)
			att := map[string]any{"id": demoID("attachment", fmt.Sprint(a["url"])), "title": a["title"], "url": a["url"], "issue": m}
			m["attachments"] = append(m["attachments"].([]any), att)
			w.root["attachments"] = append(w.root["attachments"], att)
		}
		for _, c := range asList(s["comments"]) {
			c := c.(map[string]any)
			cm := map[string]any{"id": demoID("comment", c["id"].(string)), "body": c["body"], "createdAt": at(c["createdAt"]),
				"url":  fmt.Sprintf("%s#comment-%s", m["url"], demoID("comment", c["id"].(string))[:8]),
				"user": users[fmt.Sprint(c["user"])], "issue": m, "parent": nil}
			if p, ok := c["parent"].(string); ok {
				cm["parent"] = map[string]any{"id": demoID("comment", p)}
			}
			m["comments"] = append(m["comments"].([]any), cm)
			w.root["comments"] = append(w.root["comments"], cm)
		}
		issues[ident] = m
		w.root["issues"] = append(w.root["issues"], m)
		if p := m["project"]; p != nil {
			p.(map[string]any)["issues"] = append(p.(map[string]any)["issues"].([]any), m)
		}
	}
	for _, s := range seed.Issues {
		m := issues[s["identifier"].(string)]
		if p, ok := s["parent"].(string); ok && issues[p] != nil {
			m["parent"] = issues[p]
			issues[p]["children"] = append(issues[p]["children"].([]any), m)
		}
		for _, r := range asList(s["relations"]) {
			r := r.(map[string]any)
			other := issues[fmt.Sprint(r["issue"])]
			if other == nil {
				continue
			}
			rel := map[string]any{"id": demoID("relation", m["identifier"].(string)+fmt.Sprint(r["type"])+other["identifier"].(string)),
				"type": r["type"], "issue": m, "relatedIssue": other}
			m["relations"] = append(m["relations"].([]any), rel)
			other["inverseRelations"] = append(other["inverseRelations"].([]any), rel)
		}
	}
	for _, k := range []string{"projectMilestones", "notifications", "attachments", "comments", "cycles", "projects"} {
		if w.root[k] == nil {
			w.root[k] = []any{}
		}
	}
	return w
}

func nilIfMissing(m map[string]map[string]any, key any) any {
	if v, ok := m[fmt.Sprint(key)]; ok && key != nil {
		return v
	}
	return nil
}

func asList(v any) []any {
	l, _ := v.([]any)
	return l
}

// demoAnswer resolves a GraphQL request against the demo workspace, as
// Linear would answer it.
func demoAnswer(query string, variables map[string]any) *gqlResponse {
	fail := func(msg string) *gqlResponse {
		return &gqlResponse{Errors: []gqlError{{Message: msg, Extensions: map[string]any{"code": "DEMO"}}}}
	}
	doc, err := parser.ParseQuery(&ast.Source{Input: query})
	if err != nil {
		return fail(err.Error())
	}
	if len(doc.Operations) == 0 || doc.Operations[0].Operation != ast.Query {
		return fail("demo mode is read-only: a fictional workspace, nothing is sent or changed")
	}
	// Variables as JSON gives them: numbers as float64, like a real answer.
	var vars map[string]any
	b, _ := json.Marshal(variables)
	_ = json.Unmarshal(b, &vars)
	r := &resolver{w: demoWorld(), vars: vars}
	data := map[string]any{}
	for _, sel := range doc.Operations[0].SelectionSet {
		f, ok := sel.(*ast.Field)
		if !ok || !r.included(f.Directives) {
			continue
		}
		v, err := r.rootField(f)
		if err != nil {
			return &gqlResponse{Errors: []gqlError{{Message: err.Error()}}}
		}
		data[alias(f)] = v
	}
	out, _ := json.Marshal(data)
	return &gqlResponse{Data: out}
}

type resolver struct {
	w    *world
	vars map[string]any
}

func alias(f *ast.Field) string {
	if f.Alias != "" {
		return f.Alias
	}
	return f.Name
}

func (r *resolver) args(f *ast.Field) map[string]any {
	out := map[string]any{}
	for _, a := range f.Arguments {
		if v, err := a.Value.Value(r.vars); err == nil {
			out[a.Name] = normalize(v)
		}
	}
	return out
}

// normalize makes literal and variable values alike: numbers float64.
func normalize(v any) any {
	b, _ := json.Marshal(v)
	var out any
	_ = json.Unmarshal(b, &out)
	return out
}

func (r *resolver) included(ds ast.DirectiveList) bool {
	for _, d := range ds {
		if a := d.Arguments.ForName("if"); a != nil {
			v, _ := a.Value.Value(r.vars)
			if (d.Name == "include" && v != true) || (d.Name == "skip" && v == true) {
				return false
			}
		}
	}
	return true
}

func (r *resolver) rootField(f *ast.Field) (any, error) {
	args := r.args(f)
	find := func(list, kind string, keys ...string) (any, error) {
		id := strings.ToLower(fmt.Sprint(args["id"]))
		for _, n := range r.w.root[list] {
			for _, k := range keys {
				if strings.ToLower(fmt.Sprint(n.(map[string]any)[k])) == id {
					return r.value(n, f), nil
				}
			}
		}
		return nil, fmt.Errorf("Entity not found: %s", kind)
	}
	switch f.Name {
	case "viewer":
		return r.value(r.w.viewer, f), nil
	case "organization":
		return r.value(r.w.org, f), nil
	case "issue":
		return find("issues", "Issue", "id", "identifier")
	case "project":
		return find("projects", "Project", "id", "slugId")
	case "comment":
		return find("comments", "Comment", "id")
	case "team":
		return find("teams", "Team", "id", "key")
	case "searchIssues":
		term := strings.ToLower(fmt.Sprint(args["term"]))
		hits := []any{}
		for _, n := range r.w.root["issues"] {
			m := n.(map[string]any)
			text := fmt.Sprint(m["title"], " ", m["description"])
			if args["includeComments"] == true {
				for _, c := range m["comments"].([]any) {
					text += " " + fmt.Sprint(c.(map[string]any)["body"])
				}
			}
			if strings.Contains(strings.ToLower(text), term) {
				hits = append(hits, n)
			}
		}
		return r.value(hits, f), nil
	}
	list, ok := r.w.root[f.Name]
	if !ok {
		return nil, fmt.Errorf("demo mode does not answer %s", f.Name)
	}
	return r.value(list, f), nil
}

// value resolves v for field f: a connection, an object, or a scalar.
func (r *resolver) value(v any, f *ast.Field) any {
	if len(f.SelectionSet) == 0 {
		return v
	}
	switch t := v.(type) {
	case []any:
		items := r.connection(t, r.args(f))
		page, cursor, more := items.page, items.cursor, items.more
		out := map[string]any{}
		for _, sel := range f.SelectionSet {
			sf, ok := sel.(*ast.Field)
			if !ok || !r.included(sf.Directives) {
				continue
			}
			switch sf.Name {
			case "nodes":
				nodes := []any{}
				for _, n := range page {
					nodes = append(nodes, r.object(n, sf.SelectionSet))
				}
				out[alias(sf)] = nodes
			case "pageInfo":
				out[alias(sf)] = r.object(map[string]any{"hasNextPage": more, "endCursor": cursor}, sf.SelectionSet)
			}
		}
		return out
	case map[string]any:
		return r.object(t, f.SelectionSet)
	}
	return nil
}

func (r *resolver) object(v any, set ast.SelectionSet) any {
	node, ok := v.(map[string]any)
	if !ok {
		return nil
	}
	out := map[string]any{}
	for _, sel := range set {
		switch s := sel.(type) {
		case *ast.Field:
			if r.included(s.Directives) {
				out[alias(s)] = r.value(node[s.Name], s)
			}
		case *ast.InlineFragment:
			if tn, _ := node["__typename"].(string); s.TypeCondition == "" || s.TypeCondition == tn {
				for k, x := range r.object(node, s.SelectionSet).(map[string]any) {
					out[k] = x
				}
			}
		}
	}
	return out
}

type pageOf struct {
	page   []any
	cursor any
	more   bool
}

// connection filters, orders and pages a list as Linear's connections do.
func (r *resolver) connection(all []any, args map[string]any) pageOf {
	var items []any
	for _, n := range all {
		m, _ := n.(map[string]any)
		if args["includeArchived"] != true && m != nil && m["archivedAt"] != nil {
			continue
		}
		if f, ok := args["filter"].(map[string]any); ok && !match(m, f) {
			continue
		}
		items = append(items, n)
	}
	key := func(n any, k string) string { return fmt.Sprint(n.(map[string]any)[k]) }
	switch {
	case args["sort"] != nil:
		desc := strings.Contains(fmt.Sprint(args["sort"]), "Descending")
		rank := func(n any) float64 {
			p, _ := n.(map[string]any)["priority"].(float64)
			if p == 0 {
				return 5 // no priority sorts after low, whichever way
			}
			if !desc {
				return -p
			}
			return p
		}
		sort.SliceStable(items, func(i, j int) bool { return rank(items[i]) < rank(items[j]) })
	case args["orderBy"] != nil:
		k := fmt.Sprint(args["orderBy"])
		sort.SliceStable(items, func(i, j int) bool { return key(items[i], k) > key(items[j], k) })
	}
	start := 0
	if a, ok := args["after"].(string); ok {
		start, _ = strconv.Atoi(strings.TrimPrefix(a, "c"))
	}
	first := 50
	if f, ok := args["first"].(float64); ok {
		first = int(f)
	}
	start = min(start, len(items))
	end := min(start+first, len(items))
	return pageOf{page: items[start:end], cursor: "c" + strconv.Itoa(end), more: end < len(items)}
}

var comparators = []string{"eq", "neq", "in", "nin", "eqIgnoreCase", "neqIgnoreCase", "contains", "containsIgnoreCase",
	"notContains", "startsWith", "endsWith", "gt", "gte", "lt", "lte", "null"}

// match is whether node passes a Linear filter.
func match(node map[string]any, filter map[string]any) bool {
	for k, cond := range filter {
		c, _ := cond.(map[string]any)
		switch k {
		case "and":
			for _, f := range cond.([]any) {
				if !match(node, f.(map[string]any)) {
					return false
				}
			}
			continue
		case "or":
			ok := false
			for _, f := range cond.([]any) {
				ok = ok || match(node, f.(map[string]any))
			}
			if !ok {
				return false
			}
			continue
		case "null":
			if (node == nil) != (cond == true) {
				return false
			}
			continue
		}
		if node == nil {
			return false
		}
		v := node[k]
		switch t := v.(type) {
		case []any:
			if !matchList(t, c) {
				return false
			}
		case map[string]any:
			if !match(t, c) {
				return false
			}
		default:
			if k == "id" && node["identifier"] != nil {
				// Linear's issue id filter also takes identifiers.
				if !compare(v, c) && !compare(node["identifier"], c) {
					return false
				}
				continue
			}
			if v == nil && !onlyComparators(c) {
				if !match(nil, c) {
					return false
				}
				continue
			}
			if !compare(v, c) {
				return false
			}
		}
	}
	return true
}

func onlyComparators(c map[string]any) bool {
	for k := range c {
		if !slices.Contains(comparators, k) {
			return false
		}
	}
	return true
}

func matchList(items []any, c map[string]any) bool {
	for op, f := range c {
		sub, _ := f.(map[string]any)
		switch op {
		case "some":
			ok := false
			for _, it := range items {
				ok = ok || match(it.(map[string]any), sub)
			}
			if !ok {
				return false
			}
		case "every":
			for _, it := range items {
				if !match(it.(map[string]any), sub) {
					return false
				}
			}
		case "length":
			if !compare(float64(len(items)), sub) {
				return false
			}
		}
	}
	return true
}

// compare applies comparators (eq, in, gte, null…) to a scalar.
func compare(v any, c map[string]any) bool {
	for op, want := range c {
		s, ws := fmt.Sprint(v), fmt.Sprint(want)
		var ok bool
		switch op {
		case "eq":
			ok = v != nil && s == ws
		case "neq":
			ok = s != ws
		case "eqIgnoreCase":
			ok = v != nil && strings.EqualFold(s, ws)
		case "neqIgnoreCase":
			ok = !strings.EqualFold(s, ws)
		case "in", "nin":
			found := false
			for _, x := range asList(want) {
				found = found || (v != nil && fmt.Sprint(x) == s)
			}
			ok = found == (op == "in")
		case "contains":
			ok = strings.Contains(s, ws)
		case "containsIgnoreCase":
			ok = strings.Contains(strings.ToLower(s), strings.ToLower(ws))
		case "notContains":
			ok = !strings.Contains(s, ws)
		case "startsWith":
			ok = strings.HasPrefix(s, ws)
		case "endsWith":
			ok = strings.HasSuffix(s, ws)
		case "gt", "gte", "lt", "lte":
			ok = v != nil && order(v, want, op)
		case "null":
			ok = (v == nil) == (want == true)
		default:
			return false
		}
		if !ok {
			return false
		}
	}
	return true
}

// order compares numbers as numbers and times as times.
func order(v, want any, op string) bool {
	var d float64
	if a, ok := v.(float64); ok {
		b, _ := want.(float64)
		d = a - b
	} else {
		a, err1 := time.Parse(time.RFC3339, fmt.Sprint(v))
		b, err2 := time.Parse(time.RFC3339, fmt.Sprint(want))
		if err1 != nil || err2 != nil {
			return false
		}
		d = float64(a.Sub(b))
	}
	switch op {
	case "gt":
		return d > 0
	case "gte":
		return d >= 0
	case "lt":
		return d < 0
	}
	return d <= 0
}
