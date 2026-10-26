package main

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"sync"
	"testing"

	"github.com/coder/websocket"
	"github.com/vektah/gqlparser/v2"
	"github.com/vektah/gqlparser/v2/ast"
	"github.com/vektah/gqlparser/v2/validator"
)

// linearSchema is Linear's published schema, loaded once for the fake.
var linearSchema = sync.OnceValue(func() *ast.Schema {
	data, err := os.ReadFile("../../graphql/schema.graphql")
	if err != nil {
		panic(err)
	}
	return gqlparser.MustLoadSchema(&ast.Source{Name: "schema.graphql", Input: string(data)})
})

// checkRequest validates a request as Linear would: the document against
// the schema, and the variables (filters, inputs, enums) against the
// document. queries_test.go checks the documents alone; this checks what
// each test actually sends.
func checkRequest(query string, vars map[string]any) error {
	doc, errs := gqlparser.LoadQuery(linearSchema(), query)
	if errs != nil {
		return errs
	}
	for _, op := range doc.Operations {
		if _, err := validator.VariableValues(linearSchema(), op, vars); err != nil {
			return err
		}
	}
	return nil
}

// testKey is the only key the fake accepts.
const testKey = "lin_api_test0000000000000000"

// call is one request the fake received.
type call struct {
	Op     string
	Vars   map[string]any
	Header http.Header
}

// fake is a Linear GraphQL endpoint answering by operation name. An operation
// nobody registered fails the test: every request a command sends is one the
// test expected.
type fake struct {
	t       *testing.T
	srv     *httptest.Server
	mu      sync.Mutex
	log     []call
	answers map[string]func(vars map[string]any) (int, any, map[string]string)
	raw     bool // documents come from the caller (api query): not checked
	noWS    bool // refuse WebSocket upgrades: no subscriptions
	ws      []*websocket.Conn
	wsCount int // subscriptions accepted so far
}

func newFake(t *testing.T) *fake {
	f := &fake{t: t, answers: map[string]func(map[string]any) (int, any, map[string]string){}}
	f.srv = httptest.NewServer(http.HandlerFunc(f.serve))
	t.Cleanup(f.srv.Close)
	return f
}

func (f *fake) url() string { return f.srv.URL + "/graphql" }

// on answers op with {"data": data}.
func (f *fake) on(op string, data any) *fake {
	return f.onFunc(op, func(map[string]any) (int, any, map[string]string) {
		return 200, map[string]any{"data": data}, nil
	})
}

// onFunc answers op with fn's status, body and headers.
func (f *fake) onFunc(op string, fn func(vars map[string]any) (int, any, map[string]string)) *fake {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.answers[op] = fn
	return f
}

func (f *fake) calls() []call {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([]call(nil), f.log...)
}

// ops is the operation names received, in order.
func (f *fake) ops() []string {
	var out []string
	for _, c := range f.calls() {
		out = append(out, c.Op)
	}
	return out
}

// last is the last call to op.
func (f *fake) last(op string) call {
	f.t.Helper()
	calls := f.calls()
	for i := len(calls) - 1; i >= 0; i-- {
		if calls[i].Op == op {
			return calls[i]
		}
	}
	f.t.Fatalf("no call to %s; got %v", op, f.ops())
	return call{}
}

func (f *fake) serve(w http.ResponseWriter, r *http.Request) {
	if r.Header.Get("Upgrade") == "websocket" {
		f.serveWS(w, r)
		return
	}
	var req struct {
		Query     string         `json:"query"`
		Variables map[string]any `json:"variables"`
	}
	body, _ := io.ReadAll(r.Body)
	if r.Method != http.MethodPost || json.Unmarshal(body, &req) != nil {
		http.Error(w, "bad request", http.StatusBadRequest)
		return
	}
	op := opName(req.Query)
	if err := checkRequest(req.Query, req.Variables); err != nil && !f.raw {
		f.t.Errorf("%s: Linear would reject this request: %v", op, err)
	}
	f.mu.Lock()
	f.log = append(f.log, call{op, req.Variables, r.Header.Clone()})
	answer := f.answers[op]
	f.mu.Unlock()
	w.Header().Set("Content-Type", "application/json")
	if r.Header.Get("Authorization") != testKey {
		w.WriteHeader(http.StatusBadRequest)
		_ = json.NewEncoder(w).Encode(map[string]any{"errors": []any{map[string]any{
			"message": "Authentication required", "extensions": map[string]any{"code": "AUTHENTICATION_ERROR"}}}})
		return
	}
	if answer == nil {
		f.t.Errorf("unexpected operation %q", op)
		w.WriteHeader(http.StatusInternalServerError)
		_, _ = w.Write([]byte(`{"errors":[{"message":"unexpected operation"}]}`))
		return
	}
	status, out, headers := answer(req.Variables)
	for k, v := range headers {
		w.Header().Set(k, v)
	}
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(out)
}

// linearErr is Linear's error answer.
func linearErr(msg, code string) func(map[string]any) (int, any, map[string]string) {
	return func(map[string]any) (int, any, map[string]string) {
		return 400, map[string]any{"errors": []any{map[string]any{"message": msg, "extensions": map[string]any{"code": code}}}}, nil
	}
}

// conn is a connection page.
func conn(next string, nodes ...any) map[string]any {
	if nodes == nil {
		nodes = []any{}
	}
	return map[string]any{"nodes": nodes, "pageInfo": map[string]any{"hasNextPage": next != "", "endCursor": next}}
}

// nodes is a nested connection asked for without pageInfo.
func nodes(items ...any) map[string]any {
	if items == nil {
		items = []any{}
	}
	return map[string]any{"nodes": items}
}

// issueNode is an issue as issueRow returns it.
func issueNode(identifier, title string) map[string]any {
	return map[string]any{
		"id": "id-" + identifier, "identifier": identifier, "title": title, "priority": 2, "priorityLabel": "High",
		"url": "https://linear.app/acme/issue/" + identifier, "createdAt": "2026-10-01T00:00:00.000Z",
		"updatedAt": "2026-10-02T00:00:00.000Z", "state": map[string]any{"name": "Todo", "type": "unstarted"},
		"assignee": map[string]any{"name": "Ada", "email": "ada@example.com"}, "team": map[string]any{"key": "ENG"},
		"project": nil, "cycle": nil, "labels": map[string]any{"nodes": []any{map[string]any{"name": "bug"}}},
	}
}

// serveWS speaks graphql-transport-ws as Linear does: the key comes in the
// upgrade's Authorization header; without it, subscribing closes with 4002.
func (f *fake) serveWS(w http.ResponseWriter, r *http.Request) {
	f.mu.Lock()
	refuse := f.noWS
	f.mu.Unlock()
	if refuse {
		http.Error(w, "no", http.StatusNotFound)
		return
	}
	conn, err := websocket.Accept(w, r, &websocket.AcceptOptions{Subprotocols: []string{"graphql-transport-ws"}})
	if err != nil {
		return
	}
	ctx := context.Background()
	keyed := r.Header.Get("Authorization") == testKey
	for {
		_, b, err := conn.Read(ctx)
		if err != nil {
			return
		}
		var m struct {
			Type    string `json:"type"`
			ID      string `json:"id"`
			Payload struct {
				Query string `json:"query"`
			} `json:"payload"`
		}
		_ = json.Unmarshal(b, &m)
		switch m.Type {
		case "connection_init":
			_ = conn.Write(ctx, websocket.MessageText, []byte(`{"type":"connection_ack"}`))
		case "ping":
			_ = conn.Write(ctx, websocket.MessageText, []byte(`{"type":"pong"}`))
		case "subscribe":
			if !keyed {
				_ = conn.Close(4002, "You need to authenticate to access this operation.")
				return
			}
			if err := checkRequest(m.Payload.Query, nil); err != nil {
				f.t.Errorf("subscription %s: Linear would reject it: %v", m.ID, err)
			}
			f.mu.Lock()
			f.wsCount++
			if m.ID == "updated" {
				f.ws = append(f.ws, conn)
			}
			f.mu.Unlock()
		}
	}
}

// push tells every open subscription that issue id was updated.
func (f *fake) push(id string) {
	f.mu.Lock()
	defer f.mu.Unlock()
	for _, c := range f.ws {
		msg := `{"type":"next","id":"updated","payload":{"data":{"issueUpdated":{"id":"` + id + `"}}}}`
		_ = c.Write(context.Background(), websocket.MessageText, []byte(msg))
	}
}

// dropWS closes every open subscription, as a network failure would.
func (f *fake) dropWS() {
	f.mu.Lock()
	defer f.mu.Unlock()
	for _, c := range f.ws {
		_ = c.CloseNow()
	}
	f.ws = nil
}

// subscribed is how many subscriptions the fake has accepted.
func (f *fake) subscribed() int {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.wsCount
}
