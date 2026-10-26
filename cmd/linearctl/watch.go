package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"slices"
	"strings"
	"time"

	"github.com/coder/websocket"
	"github.com/spf13/cobra"
)

// watch streams changes to the issues a filter matches, one JSON document
// per line. Linear's GraphQL subscriptions (WebSocket, graphql-transport-ws,
// undocumented but working with a personal API key, verified 2026-10-26)
// say which issues changed; each batch of changed ids is then checked
// against the filter with one issue list request, so a change that makes an
// issue stop matching is reported as a remove. When subscriptions are not
// available it polls instead.

const (
	snapshotPages = 10 // known ids at start: up to 2,500 issues
	reconcileSkew = 5 * time.Second
)

type watcher struct {
	cmd        *cobra.Command
	cl         *client
	filter     map[string]any // nil: every issue
	fields     []string
	known      map[string]string // id → identifier: issues known to match
	everything bool              // the snapshot was cut short: report every non-match
	mode       string
	since      time.Time // changes from here on are covered
}

func addWatch(root *cobra.Command) {
	c := &cobra.Command{
		Use:   "watch",
		Short: "Stream changes to the issues a filter matches, one JSON line each",
		Long: "Stream changes to the issues a filter matches. With --json each line is one event:\n" +
			"  ready      {mode, since, known, everything}: up to date from since\n" +
			"  upsert     {issue}: an issue that matches, in issue list's row shape\n" +
			"  remove     {id, identifier, reason}: it stopped matching; reason is filter,\n" +
			"             archived or deleted\n" +
			"  heartbeat  {at, mode, rateLimit}: every --heartbeat seconds\n" +
			"  gap        {from, to, reason}: the stream was down (reconnect) or fell back\n" +
			"             to polling (fallback); what changed meanwhile follows as upserts\n" +
			"It runs until interrupted (exit 130) or a request fails, which ends it with the\n" +
			"usual exit codes (5 auth, 6 rate-limited, 1 failed) and a last {error} line.\n\n" +
			"Linear's subscriptions name the changed issues; linearctl checks each batch\n" +
			"against the filter with one request (at most one per 2 s) and reconciles every\n" +
			"--reconcile seconds. --mode poll asks Linear every --poll-interval seconds instead.\n" +
			"Removes are reported for issues known to match: up to 2,500 found at start, and\n" +
			"any since; beyond that (everything: true) every non-matching change is a remove.",
		Example: "  linearctl watch --team ENG --json\n" +
			"  linearctl watch --assignee me --state-type started --since 2026-10-26T09:00:00Z --json",
		Args: usageArgs(cobra.NoArgs),
		RunE: func(cmd *cobra.Command, _ []string) error {
			filter, err := listFilter(cmd)
			if err != nil {
				return err
			}
			fields, err := fieldsFlag(cmd, issueRowFields)
			if err != nil {
				return err
			}
			mode, _ := cmd.Flags().GetString("mode")
			if !slices.Contains([]string{"auto", "subscription", "poll"}, mode) {
				return usageError{fmt.Errorf("--mode must be auto, subscription or poll; got %q", mode)}
			}
			var since time.Time
			if s, _ := cmd.Flags().GetString("since"); s != "" {
				t, err := when(s)
				if err != nil {
					return usageError{fmt.Errorf("--since: %s", strings.TrimPrefix(err.Error(), "--updated-since "))}
				}
				since, _ = time.Parse(time.RFC3339, t)
			}
			seconds := func(name string, min int) (time.Duration, error) {
				n := cmd.Flags().Lookup(name).Value.(*wholeNumber).n
				if n < min {
					return 0, usageError{fmt.Errorf("--%s is at least %d", name, min)}
				}
				return time.Duration(n) * time.Second, nil
			}
			heartbeat, err := seconds("heartbeat", 1)
			if err != nil {
				return err
			}
			reconcile, err := seconds("reconcile", 10)
			if err != nil {
				return err
			}
			poll, err := seconds("poll-interval", 10)
			if err != nil {
				return err
			}
			batch := time.Duration(cmd.Flags().Lookup("batch-ms").Value.(*wholeNumber).n) * time.Millisecond
			cl, err := newClient()
			if err != nil {
				return err
			}
			w := &watcher{cmd: cmd, cl: cl, filter: filter, fields: fields, known: map[string]string{}}
			return w.run(mode, since, heartbeat, reconcile, poll, batch)
		},
	}
	filterFlags(c)
	c.Flags().String("since", "", "first replay changes since `WHEN` (a date, RFC 3339 time, 7d, 24h)")
	c.Flags().String("mode", "auto", "`MODE`: auto (subscribe, else poll), subscription, poll")
	c.Flags().Var(&wholeNumber{n: 30}, "heartbeat", "print a heartbeat every `N` seconds")
	c.Flags().Var(&wholeNumber{n: 300}, "reconcile", "with subscriptions, catch up on missed changes every `N` s")
	c.Flags().Var(&wholeNumber{n: 60}, "poll-interval", "when polling, ask Linear every `N` seconds")
	c.Flags().Var(&wholeNumber{n: 2000}, "batch-ms", "check changed issues at most every `MS` milliseconds")
	_ = c.Flags().MarkHidden("batch-ms") // for tests; the default protects the rate limit
	addFieldsFlag(c, issueRowFields)
	root.AddCommand(c)
}

// emit prints one event: a JSON line with --json, else a line of text.
func (w *watcher) emit(event string, doc map[string]any) {
	doc["event"] = event
	if asJSON {
		b, _ := json.Marshal(doc)
		fmt.Fprintln(w.cmd.OutOrStdout(), string(b))
		return
	}
	out := w.cmd.OutOrStdout()
	switch event {
	case "upsert":
		it := doc["issue"].(map[string]any)
		fmt.Fprintf(out, "upsert %s\t%s\t%s\n", str(it, "identifier"), printable(str(it, "state.name")), printable(str(it, "title")))
	case "remove":
		fmt.Fprintf(out, "remove %s (%s)\n", doc["identifier"], doc["reason"])
	default:
		b, _ := json.Marshal(doc)
		fmt.Fprintf(out, "%s %s\n", event, b)
	}
}

func (w *watcher) upsert(it map[string]any) {
	w.known[str(it, "id")] = str(it, "identifier")
	w.emit("upsert", map[string]any{"issue": pick(flatten(it).(map[string]any), w.fields)})
}

// and is the watcher's filter with one more condition.
func (w *watcher) and(cond map[string]any) map[string]any {
	if w.filter == nil {
		return cond
	}
	return map[string]any{"and": []any{w.filter, cond}}
}

// snapshot learns which issues match now, to know what a remove is.
func (w *watcher) snapshot() error {
	cursor := ""
	for range snapshotPages {
		var out struct {
			Issues page `json:"issues"`
		}
		vars := map[string]any{"filter": w.filter, "first": pageSize, "after": optional(cursor)}
		if err := w.cl.query(qWatchIDs, vars, &out); err != nil {
			return err
		}
		for _, n := range out.Issues.Nodes {
			w.known[str(n, "id")] = str(n, "identifier")
		}
		if !out.Issues.PageInfo.HasNextPage {
			return nil
		}
		cursor = out.Issues.PageInfo.EndCursor
	}
	w.everything = true
	return nil
}

// gone reports issues that no longer match, with why.
func (w *watcher) gone(ids []string) error {
	var report []string
	for _, id := range ids {
		if _, ok := w.known[id]; ok || w.everything {
			report = append(report, id)
		}
	}
	if len(report) == 0 {
		return nil
	}
	var out struct {
		Issues struct {
			Nodes []struct {
				ID         string  `json:"id"`
				Identifier string  `json:"identifier"`
				ArchivedAt *string `json:"archivedAt"`
				Trashed    *bool   `json:"trashed"`
			} `json:"nodes"`
		} `json:"issues"`
	}
	if err := w.cl.query(qWatchGone, map[string]any{"ids": report}, &out); err != nil {
		return err
	}
	for _, id := range report {
		reason, ident := "deleted", w.known[id]
		for _, n := range out.Issues.Nodes {
			if n.ID != id {
				continue
			}
			ident = n.Identifier
			switch {
			case n.Trashed != nil && *n.Trashed:
				reason = "deleted"
			case n.ArchivedAt != nil:
				reason = "archived"
			default:
				reason = "filter"
			}
		}
		delete(w.known, id)
		w.emit("remove", map[string]any{"id": id, "identifier": optional(ident), "reason": reason})
	}
	return nil
}

// check sends one request for a batch of changed ids: those that match are
// upserts, the rest may be removes.
func (w *watcher) check(ids []string) error {
	var out struct {
		Issues page `json:"issues"`
	}
	vars := map[string]any{"filter": w.and(map[string]any{"id": map[string]any{"in": ids}}), "first": pageSize}
	if err := w.cl.query(qIssueList, vars, &out); err != nil {
		return err
	}
	matched := map[string]bool{}
	for _, it := range out.Issues.Nodes {
		matched[str(it, "id")] = true
		w.upsert(it)
	}
	var rest []string
	for _, id := range ids {
		if !matched[id] {
			rest = append(rest, id)
		}
	}
	return w.gone(rest)
}

// reconcile catches up on everything updated since from: what matches is
// upserted, what changed and no longer matches is removed.
func (w *watcher) reconcile(from time.Time) error {
	started := time.Now()
	gte := map[string]any{"updatedAt": map[string]any{"gte": from.UTC().Format(time.RFC3339)}}
	items, _, err := collect(pageSize*snapshotPages, "", func(first int, after string) (page, error) {
		var out struct {
			Issues page `json:"issues"`
		}
		err := w.cl.query(qIssueList, map[string]any{"filter": w.and(gte), "first": first, "after": optional(after)}, &out)
		return out.Issues, err
	})
	if err != nil {
		return err
	}
	matched := map[string]bool{}
	for _, it := range items {
		matched[str(it, "id")] = true
		w.upsert(it)
	}
	var changed struct {
		Issues struct {
			Nodes []struct {
				ID string `json:"id"`
			} `json:"nodes"`
		} `json:"issues"`
	}
	if err := w.cl.query(qWatchChanged, map[string]any{"filter": gte}, &changed); err != nil {
		return err
	}
	var rest []string
	for _, n := range changed.Issues.Nodes {
		if !matched[n.ID] {
			rest = append(rest, n.ID)
		}
	}
	if err := w.gone(rest); err != nil {
		return err
	}
	w.since = started.Add(-reconcileSkew)
	return nil
}

func (w *watcher) heartbeat() {
	doc := map[string]any{"at": time.Now().UTC().Format(time.RFC3339), "mode": w.mode, "rateLimit": nil}
	if w.cl.header != nil {
		doc["rateLimit"] = w.cl.rateLimit()
	}
	w.emit("heartbeat", doc)
}

func (w *watcher) run(mode string, since time.Time, heartbeat, reconcile, poll, batch time.Duration) error {
	start := time.Now()
	var sub *subscription
	if mode != "poll" {
		var err error
		if sub, err = subscribe(w.cl.key); err != nil {
			if mode == "subscription" {
				return err
			}
			fmt.Fprintf(stderr, "linearctl: subscriptions unavailable (%s); polling every %s\n", cause(err), poll)
		}
	}
	w.mode = map[bool]string{true: "subscription", false: "poll"}[sub != nil]
	if err := w.snapshot(); err != nil {
		return err
	}
	w.since = start.Add(-reconcileSkew)
	if !since.IsZero() {
		if err := w.reconcile(since); err != nil {
			return err
		}
		w.since = start.Add(-reconcileSkew)
	}
	w.emit("ready", map[string]any{"mode": w.mode, "since": w.since.UTC().Format(time.RFC3339),
		"known": len(w.known), "everything": w.everything})

	beat := time.NewTicker(heartbeat)
	defer beat.Stop()
	every := reconcile
	if sub == nil {
		every = poll
	}
	catchUp := time.NewTicker(every)
	defer catchUp.Stop()
	flush := time.NewTimer(batch)
	flush.Stop()
	pending := []string{}
	var events <-chan string
	var failed <-chan error
	if sub != nil {
		events, failed = sub.events, sub.failed
	}
	backoff := time.Second
	for {
		select {
		case id := <-events:
			if !slices.Contains(pending, id) {
				pending = append(pending, id)
			}
			if len(pending) == 1 {
				flush.Reset(batch)
			}
			if len(pending) >= pageSize {
				flush.Reset(0)
			}
		case <-flush.C:
			if len(pending) > 0 {
				ids := pending
				pending = []string{}
				if err := w.check(ids); err != nil {
					return err
				}
			}
		case <-catchUp.C:
			if err := w.reconcile(w.since); err != nil {
				return err
			}
		case <-beat.C:
			if sub != nil && time.Since(sub.lastSeen()) > 2*heartbeat+5*time.Second {
				sub.close()
				failed = closedWith(errors.New("no message from Linear for too long"))
				continue
			}
			if sub != nil {
				sub.ping()
			}
			w.heartbeat()
		case err := <-failed:
			// The stream is down: reconnect, then replay the gap.
			down := time.Now()
			fmt.Fprintf(stderr, "linearctl: subscription lost (%s); reconnecting\n", cause(err))
			sub = nil
			for tries := 0; sub == nil && tries < 5; tries++ {
				time.Sleep(backoff)
				backoff = min(2*backoff, 30*time.Second)
				sub, _ = subscribe(w.cl.key)
			}
			reason := "reconnect"
			if sub == nil {
				reason = "fallback"
				w.mode = "poll"
				events, failed = nil, nil
				catchUp.Reset(poll)
			} else {
				backoff = time.Second
				events, failed = sub.events, sub.failed
			}
			w.emit("gap", map[string]any{"from": w.since.UTC().Format(time.RFC3339), "to": time.Now().UTC().Format(time.RFC3339),
				"reason": reason, "mode": w.mode, "downFor": time.Since(down).Round(time.Second).String()})
			if err := w.reconcile(w.since); err != nil {
				return err
			}
		}
	}
}

// subscription is a graphql-transport-ws connection subscribed to issue
// changes. events carries the ids of changed issues.
type subscription struct {
	conn   *websocket.Conn
	cancel context.CancelFunc
	events chan string
	failed chan error
	seen   chan time.Time
	last   time.Time
}

func (s *subscription) lastSeen() time.Time {
	for {
		select {
		case t := <-s.seen:
			s.last = t
		default:
			return s.last
		}
	}
}

func (s *subscription) ping() {
	_ = s.conn.Write(context.Background(), websocket.MessageText, []byte(`{"type":"ping"}`))
}

func (s *subscription) close() {
	s.cancel()
	_ = s.conn.CloseNow()
}

func closedWith(err error) <-chan error {
	c := make(chan error, 1)
	c <- err
	return c
}

// wsURL is the GraphQL endpoint's WebSocket form.
func wsURL() string {
	u := endpoint()
	if strings.HasPrefix(u, "https://") {
		return "wss://" + strings.TrimPrefix(u, "https://")
	}
	return "ws://" + strings.TrimPrefix(u, "http://")
}

// subscribe opens a subscription to issue changes. The key goes in the
// Authorization header of the upgrade: Linear rejects it in connection_init.
func subscribe(key string) (*subscription, error) {
	ctx, cancel := context.WithCancel(context.Background())
	dial, dialCancel := context.WithTimeout(ctx, time.Duration(timeout.n)*time.Second)
	defer dialCancel()
	conn, _, err := websocket.Dial(dial, wsURL(), &websocket.DialOptions{
		Subprotocols: []string{"graphql-transport-ws"},
		HTTPHeader:   http.Header{"Authorization": {key}, "User-Agent": {"linearctl/" + version}},
	})
	if err != nil {
		cancel()
		return nil, err
	}
	conn.SetReadLimit(1 << 20)
	write := func(v any) error {
		b, _ := json.Marshal(v)
		return conn.Write(dial, websocket.MessageText, b)
	}
	if err := write(map[string]any{"type": "connection_init", "payload": map[string]any{}}); err != nil {
		cancel()
		return nil, err
	}
	var ack struct{ Type string }
	if _, b, err := conn.Read(dial); err != nil || json.Unmarshal(b, &ack) != nil || ack.Type != "connection_ack" {
		cancel()
		_ = conn.CloseNow()
		return nil, fmt.Errorf("no connection_ack from %s", wsURL())
	}
	for id, doc := range map[string]string{"created": qWatchCreated, "updated": qWatchUpdated,
		"archived": qWatchArchived, "unarchived": qWatchUnarchived} {
		if err := write(map[string]any{"id": id, "type": "subscribe", "payload": map[string]any{"query": doc}}); err != nil {
			cancel()
			return nil, err
		}
	}
	s := &subscription{conn: conn, cancel: cancel, events: make(chan string, 1024), failed: make(chan error, 1),
		seen: make(chan time.Time, 16), last: time.Now()}
	go s.read(ctx)
	return s, nil
}

func (s *subscription) read(ctx context.Context) {
	for {
		_, b, err := s.conn.Read(ctx)
		if err != nil {
			s.failed <- err
			return
		}
		select {
		case s.seen <- time.Now():
		default:
		}
		var m struct {
			Type    string          `json:"type"`
			ID      string          `json:"id"`
			Payload json.RawMessage `json:"payload"`
		}
		if json.Unmarshal(b, &m) != nil {
			continue
		}
		switch m.Type {
		case "next":
			var p struct {
				Data map[string]struct {
					ID string `json:"id"`
				} `json:"data"`
			}
			if json.Unmarshal(m.Payload, &p) == nil {
				for _, v := range p.Data {
					if v.ID != "" {
						s.events <- v.ID
					}
				}
			}
		case "ping":
			_ = s.conn.Write(ctx, websocket.MessageText, []byte(`{"type":"pong"}`))
		case "error", "complete":
			s.failed <- fmt.Errorf("subscription %s ended: %s %s", m.ID, m.Type, printable(string(m.Payload)))
			s.close()
			return
		}
	}
}
