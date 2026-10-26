package main

import (
	"errors"
	"net/http"
	"net/http/httptest"
	"strconv"
	"strings"
	"testing"
	"time"
)

// These call the client in-process; they set LINEARCTL_API_URL, so they do
// not run in parallel.

func TestClientSendsTheKeyAndDecodes(t *testing.T) {
	api := newFake(t).on("Viewer", map[string]any{"viewer": map[string]any{"id": "u1", "name": "Ada"}, "organization": map[string]any{"name": "Acme"}})
	t.Setenv("LINEARCTL_API_URL", api.url())
	var v viewer
	if err := newClientWith(testKey, "env").query(qViewer, nil, &v); err != nil || v.Viewer.Name != "Ada" {
		t.Fatalf("%v %+v", err, v)
	}
	h := api.last("Viewer").Header
	if h.Get("Authorization") != testKey || h.Get("public-file-urls-expire-in") != "" || !strings.HasPrefix(h.Get("User-Agent"), "linearctl/") {
		t.Errorf("headers %v", h)
	}
}

func TestClientSignsFileURLsWhenAsked(t *testing.T) {
	api := newFake(t).on("Viewer", map[string]any{})
	t.Setenv("LINEARCTL_API_URL", api.url())
	c := newClientWith(testKey, "env")
	c.signedURLs = 300
	must(t, c.query(qViewer, nil, nil))
	if got := api.last("Viewer").Header.Get("public-file-urls-expire-in"); got != "300" {
		t.Errorf("header %q", got)
	}
}

func TestClientMapsFailures(t *testing.T) {
	reset := time.Now().Add(10 * time.Minute).Truncate(time.Millisecond)
	cases := []struct {
		name   string
		answer func(map[string]any) (int, any, map[string]string)
		key    string
		check  func(error) bool
	}{
		{"wrong key", nil, "lin_api_wrong000000000000000", func(err error) bool { return errors.As(err, new(authError)) }},
		{"rate limited", func(map[string]any) (int, any, map[string]string) {
			return 400, map[string]any{"errors": []any{map[string]any{"message": "Rate limit exceeded", "extensions": map[string]any{"code": "RATELIMITED"}}}},
				map[string]string{"X-RateLimit-Requests-Remaining": "0", "X-RateLimit-Requests-Reset": strconv.FormatInt(reset.UnixMilli(), 10)}
		}, testKey, func(err error) bool {
			var l limitError
			return errors.As(err, &l) && l.retryAt.Equal(reset)
		}},
		{"429", func(map[string]any) (int, any, map[string]string) { return 429, map[string]any{}, nil }, testKey,
			func(err error) bool { return errors.As(err, new(limitError)) }},
		{"graphql error", linearErr("Entity not found: Issue", "INVALID_INPUT"), testKey, func(err error) bool {
			var le linearError
			return errors.As(err, &le) && le.notFound && strings.Contains(notFound(err, "issue ENG-9").Error(), "not found: issue ENG-9")
		}},
		{"presentable message", func(map[string]any) (int, any, map[string]string) {
			return 400, map[string]any{"errors": []any{map[string]any{"message": "x", "extensions": map[string]any{"userPresentableMessage": "Title is too long\x1b[31m"}}}}, nil
		}, testKey, func(err error) bool { return err.Error() == "Linear: Title is too long?[31m" }},
	}
	for _, c := range cases {
		api := newFake(t)
		if c.answer != nil {
			api.onFunc("Viewer", c.answer)
		}
		t.Setenv("LINEARCTL_API_URL", api.url())
		if err := newClientWith(c.key, "env").query(qViewer, nil, nil); !c.check(err) {
			t.Errorf("%s: %v (%T)", c.name, err, err)
		}
	}
}

func TestClientNamesTheHostOnTransportFailures(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(502)
		_, _ = w.Write([]byte("<html>bad gateway</html>"))
	}))
	defer srv.Close()
	t.Setenv("LINEARCTL_API_URL", srv.URL)
	if err := newClientWith(testKey, "env").query(qViewer, nil, nil); err == nil || !strings.Contains(err.Error(), "HTTP 502 without JSON") {
		t.Errorf("502: %v", err)
	}
	t.Setenv("LINEARCTL_API_URL", "http://127.0.0.1:9/graphql")
	if err := newClientWith(testKey, "env").query(qViewer, nil, nil); err == nil || !strings.Contains(err.Error(), "cannot reach 127.0.0.1:9") {
		t.Errorf("unreachable: %v", err)
	}
}

func TestClientTimesOut(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { time.Sleep(1500 * time.Millisecond) }))
	defer srv.Close()
	t.Setenv("LINEARCTL_API_URL", srv.URL)
	old := timeout.n
	timeout.n = 1
	defer func() { timeout.n = old }()
	if err := newClientWith(testKey, "env").query(qViewer, nil, nil); err == nil || !strings.Contains(err.Error(), "raise --timeout") {
		t.Errorf("%v", err)
	}
}

func TestRateLimitHeaders(t *testing.T) {
	c := &client{header: http.Header{}}
	c.header.Set("X-RateLimit-Requests-Remaining", "2400")
	c.header.Set("X-RateLimit-Requests-Limit", "2500")
	c.header.Set("X-RateLimit-Complexity-Remaining", "nope")
	got := c.rateLimit()
	if r, _ := got["requests"].(map[string]any); r["remaining"] != int64(2400) || r["limit"] != int64(2500) {
		t.Errorf("%v", got)
	}
	if _, ok := got["complexity"]; ok {
		t.Errorf("an unparsable header became a value: %v", got)
	}
}
