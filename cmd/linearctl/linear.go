package main

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime"
	"strconv"
	"strings"
	"sync"
	"time"
)

// The API and the file host. LINEARCTL_API_URL and LINEARCTL_UPLOADS_URL
// replace them for tests; nothing else should set them, since the API key goes
// wherever they point.
const (
	apiURL     = "https://api.linear.app/graphql"
	uploadsURL = "https://uploads.linear.app"
	// Linear's cap on a page ("first must not be greater than 250", verified
	// 2026-10-26); a 250-issue page measured 12 complexity points of 10,000.
	pageSize = 250
)

func endpoint() string {
	if u := os.Getenv("LINEARCTL_API_URL"); u != "" {
		return u
	}
	return apiURL
}

func uploadsOrigin() string {
	if u := os.Getenv("LINEARCTL_UPLOADS_URL"); u != "" {
		return strings.TrimSuffix(u, "/")
	}
	return uploadsURL
}

// viewer is the answer to qViewer.
type viewer struct {
	Viewer struct {
		ID    string `json:"id"`
		Name  string `json:"name"`
		Email string `json:"email"`
	} `json:"viewer"`
	Organization struct {
		Name   string `json:"name"`
		URLKey string `json:"urlKey"`
	} `json:"organization"`
}

type gqlError struct {
	Message    string         `json:"message"`
	Path       []any          `json:"path,omitempty"`
	Extensions map[string]any `json:"extensions,omitempty"`
}

type gqlResponse struct {
	Data   json.RawMessage `json:"data"`
	Errors []gqlError      `json:"errors,omitempty"`
}

// client sends GraphQL documents with one API key.
type client struct {
	key        string
	source     string // "env", "keychain" or "file"
	http       *http.Client
	signedURLs int         // seconds; 0 leaves file URLs as they are
	header     http.Header // the last response's headers
	requests   int         // sent so far, for --meta
}

// lastClient is the run's client, whose headers --meta reports.
var lastClient *client

// newClient finds the API key; without one it is an auth error (exit 5).
func newClient() (*client, error) {
	key, source, err := apiKey()
	if err != nil {
		return nil, err
	}
	if key == "" {
		return nil, authError{errors.New("no API key: set LINEAR_API_KEY, or pipe one to 'linearctl auth login --yes'")}
	}
	return newClientWith(key, source), nil
}

func newClientWith(key, source string) *client {
	lastClient = &client{key: key, source: source, http: &http.Client{Timeout: time.Duration(timeout.n) * time.Second}}
	return lastClient
}

// query sends doc and decodes its data into out. Any GraphQL error fails it.
func (c *client) query(doc string, vars map[string]any, out any) error {
	resp, err := c.raw(doc, vars)
	if err != nil {
		return err
	}
	if len(resp.Errors) > 0 {
		return linearFailure(resp.Errors)
	}
	if out == nil {
		return nil
	}
	if err := json.Unmarshal(resp.Data, out); err != nil {
		return fmt.Errorf("unexpected answer from Linear: %s", err)
	}
	return nil
}

var waitNote sync.Once

// raw sends doc and returns Linear's answer, errors included. Transport, auth
// and rate-limit failures are errors; GraphQL errors are left to the caller.
func (c *client) raw(doc string, vars map[string]any) (*gqlResponse, error) {
	return c.send(map[string]any{"query": doc, "variables": vars})
}

// send posts a whole GraphQL request body ({query, variables, operationName}).
func (c *client) send(request map[string]any) (*gqlResponse, error) {
	body, _ := json.Marshal(request)
	req, err := http.NewRequestWithContext(context.Background(), http.MethodPost, endpoint(), bytes.NewReader(body))
	if err != nil {
		return nil, err
	}
	// A personal API key goes as is; an OAuth token would need "Bearer ".
	req.Header.Set("Authorization", c.key)
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("User-Agent", "linearctl/"+version)
	if c.signedURLs > 0 {
		req.Header.Set("public-file-urls-expire-in", strconv.Itoa(c.signedURLs))
	}
	c.requests++
	res, err := c.do(req)
	if err != nil {
		return nil, err
	}
	defer res.Body.Close()
	c.header = res.Header
	data, err := io.ReadAll(io.LimitReader(res.Body, 64<<20))
	if err != nil {
		return nil, fmt.Errorf("reading Linear's answer: %s", cause(err))
	}
	var out gqlResponse
	jsonErr := json.Unmarshal(data, &out)
	for _, e := range out.Errors {
		switch e.Extensions["code"] {
		case "AUTHENTICATION_ERROR":
			return nil, authError{errors.New("Linear rejected the API key; pipe a new one to 'linearctl auth login --yes'")}
		case "RATELIMITED":
			return nil, c.limited()
		}
	}
	switch {
	case res.StatusCode == http.StatusUnauthorized:
		return nil, authError{errors.New("Linear rejected the API key; pipe a new one to 'linearctl auth login --yes'")}
	case res.StatusCode == http.StatusTooManyRequests:
		return nil, c.limited()
	case jsonErr != nil:
		return nil, fmt.Errorf("Linear answered HTTP %d without JSON", res.StatusCode)
	case res.StatusCode >= 400 && len(out.Errors) == 0:
		return nil, fmt.Errorf("Linear answered HTTP %d", res.StatusCode)
	}
	return &out, nil
}

// do sends req and says on stderr what it is waiting on once a request takes
// more than 2 s (rubric P2).
func (c *client) do(req *http.Request) (*http.Response, error) {
	slow := time.AfterFunc(2*time.Second, func() {
		waitNote.Do(func() { fmt.Fprintf(os.Stderr, "linearctl: waiting on %s\n", req.URL.Host) })
	})
	defer slow.Stop()
	res, err := c.http.Do(req)
	if err != nil {
		var uerr *url.Error
		if errors.As(err, &uerr) && uerr.Timeout() {
			return nil, fmt.Errorf("%s did not answer within %d s; raise --timeout", req.URL.Host, timeout.n)
		}
		return nil, fmt.Errorf("cannot reach %s: %s", req.URL.Host, cause(err))
	}
	return res, nil
}

// limited is a rate-limit error with the latest reset among the budgets
// that are spent, from Linear's X-RateLimit-* headers.
func (c *client) limited() error {
	var at time.Time
	for _, kind := range []string{"Requests", "Complexity", "Endpoint-Requests"} {
		if c.header.Get("X-RateLimit-"+kind+"-Remaining") != "0" {
			continue
		}
		if ms, err := strconv.ParseInt(c.header.Get("X-RateLimit-"+kind+"-Reset"), 10, 64); err == nil {
			if t := time.UnixMilli(ms); t.After(at) {
				at = t
			}
		}
	}
	msg := "Linear's rate limit is spent"
	if !at.IsZero() {
		msg += "; retry after " + at.UTC().Format(time.RFC3339)
	}
	return limitError{errors.New(msg), at}
}

// meta is what --meta adds to a result: the requests this run sent, and
// the budget left after the last one, from its headers.
func (c *client) meta() map[string]any {
	m := map[string]any{"requests": c.requests, "rateLimit": c.rateLimit(), "complexity": nil}
	if n, err := strconv.ParseFloat(c.header.Get("X-Complexity"), 64); err == nil {
		m["complexity"] = n
	}
	return m
}

// rateLimit is the budget left after the last request, from its headers.
func (c *client) rateLimit() map[string]any {
	out := map[string]any{}
	for _, kind := range []string{"Requests", "Complexity"} {
		n, err := strconv.ParseInt(c.header.Get("X-RateLimit-"+kind+"-Remaining"), 10, 64)
		if err != nil {
			continue
		}
		entry := map[string]any{"remaining": n}
		if limit, err := strconv.ParseInt(c.header.Get("X-RateLimit-"+kind+"-Limit"), 10, 64); err == nil {
			entry["limit"] = limit
		}
		if ms, err := strconv.ParseInt(c.header.Get("X-RateLimit-"+kind+"-Reset"), 10, 64); err == nil {
			entry["resetAt"] = time.UnixMilli(ms).UTC().Format(time.RFC3339)
		}
		out[strings.ToLower(kind)] = entry
	}
	return out
}

// signedUntil is when file URLs signed in the next request stop working:
// Linear signs them for signedURLs seconds from when it answers, so the
// time taken before sending can only make this early, never late.
func (c *client) signedUntil() any {
	if c.signedURLs == 0 {
		return nil
	}
	return time.Now().Add(time.Duration(c.signedURLs) * time.Second).UTC().Format(time.RFC3339)
}

// linearFailure is Linear's first error, in the words it offers people.
func linearFailure(errs []gqlError) error {
	e := errs[0]
	msg := e.Message
	if m, ok := e.Extensions["userPresentableMessage"].(string); ok && m != "" {
		msg = m
	}
	return linearError{msg: printable(msg), notFound: strings.Contains(strings.ToLower(e.Message), "not found")}
}

type linearError struct {
	msg      string
	notFound bool
}

func (e linearError) Error() string { return "Linear: " + e.msg }

// notFound turns Linear's "Entity not found" into one naming what was looked
// for (rubric A4).
func notFound(err error, what string) error {
	var le linearError
	if errors.As(err, &le) && le.notFound {
		return fmt.Errorf("not found: %s (or this API key cannot see it)", what)
	}
	return err
}

// --- API key storage ---------------------------------------------------------

// keyPattern is what a stored key may look like. It also keeps the key safe
// to hand to `security -i`, which parses its input like a shell line.
var keyPattern = regexp.MustCompile(`^[A-Za-z0-9_-]{16,256}$`)

// apiKey is LINEAR_API_KEY, else the stored key. An empty key with no error
// means none is set anywhere.
func apiKey() (key, source string, err error) {
	if k := os.Getenv("LINEAR_API_KEY"); k != "" {
		return strings.TrimSpace(k), "env", nil
	}
	return storedKey()
}

// The Keychain's command-line tool; tests replace both.
var (
	hasKeychain = runtime.GOOS == "darwin"
	securityBin = "/usr/bin/security"
)

// useKeychain: the macOS Keychain, unless LINEARCTL_HOME isolates this run
// (tests, experiments) or there is no `security`.
func useKeychain() bool {
	if !hasKeychain || os.Getenv("LINEARCTL_HOME") != "" {
		return false
	}
	_, err := os.Stat(securityBin)
	return err == nil
}

const keychainService, keychainAccount = "linearctl", "api-key"

func credentialsFile() (dir, name string, err error) {
	dir = home("config")
	if dir == "" {
		return "", "", errors.New("no home directory; set LINEARCTL_HOME or XDG_CONFIG_HOME")
	}
	return dir, "credentials.json", nil
}

func storedKey() (string, string, error) {
	if useKeychain() {
		out, err := exec.Command(securityBin, "find-generic-password",
			"-s", keychainService, "-a", keychainAccount, "-w").Output()
		if err != nil {
			var exit *exec.ExitError
			if errors.As(err, &exit) && exit.ExitCode() == 44 { // errSecItemNotFound
				return "", "keychain", nil
			}
			return "", "keychain", fmt.Errorf("cannot read the Keychain: %s", cause(err))
		}
		return strings.TrimSpace(string(out)), "keychain", nil
	}
	dir, name, err := credentialsFile()
	if err != nil {
		return "", "file", err
	}
	data, err := os.ReadFile(filepath.Join(dir, name))
	if errors.Is(err, os.ErrNotExist) {
		return "", "file", nil
	}
	if err != nil {
		return "", "file", fmt.Errorf("cannot read %s: %s", filepath.Join(dir, name), cause(err))
	}
	var creds struct {
		APIKey string `json:"apiKey"`
	}
	if json.Unmarshal(data, &creds) != nil {
		return "", "file", fmt.Errorf("%s is not valid JSON; remove it and log in again", filepath.Join(dir, name))
	}
	return creds.APIKey, "file", nil
}

// storeKey saves key where storedKey reads it. The key never goes in argv:
// `security -i` reads its command from stdin.
func storeKey(key string) error {
	if !keyPattern.MatchString(key) {
		return errors.New("refusing to store a key with unexpected characters")
	}
	if useKeychain() {
		cmd := exec.Command(securityBin, "-i")
		cmd.Stdin = strings.NewReader(fmt.Sprintf("add-generic-password -U -s %s -a %s -w %s\n", keychainService, keychainAccount, key))
		if out, err := cmd.CombinedOutput(); err != nil {
			return fmt.Errorf("cannot write the Keychain: %s", strings.TrimSpace(strings.ReplaceAll(string(out), key, "…")))
		}
		// `security -i` exits 0 whatever happened; read it back to know.
		if got, _, err := storedKey(); err != nil || got != key {
			return errors.New("cannot write the Keychain: the key did not read back")
		}
		return nil
	}
	dir, name, err := credentialsFile()
	if err != nil {
		return err
	}
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return fmt.Errorf("cannot create %s: %s", dir, cause(err))
	}
	data, _ := json.MarshalIndent(map[string]string{"apiKey": key}, "", "  ")
	return withLock(dir, name, func() error { return replaceFile(dir, name, append(data, '\n')) })
}

// forgetKey removes the stored key; false when there was none.
func forgetKey() (bool, error) {
	if useKeychain() {
		out, err := exec.Command(securityBin, "delete-generic-password",
			"-s", keychainService, "-a", keychainAccount).CombinedOutput()
		var exit *exec.ExitError
		if errors.As(err, &exit) && exit.ExitCode() == 44 {
			return false, nil
		}
		if err != nil {
			return false, fmt.Errorf("cannot write the Keychain: %s", strings.TrimSpace(string(out)))
		}
		return true, nil
	}
	dir, name, err := credentialsFile()
	if err != nil {
		return false, err
	}
	path, err := inside(dir, name)
	if errors.Is(err, os.ErrNotExist) {
		return false, nil
	}
	if err != nil {
		if _, statErr := os.Stat(dir); errors.Is(statErr, os.ErrNotExist) {
			return false, nil
		}
		return false, err
	}
	removed := false
	err = withLock(dir, name, func() error {
		err := os.Remove(path)
		if errors.Is(err, os.ErrNotExist) {
			return nil
		}
		removed = err == nil
		return err
	})
	if err != nil {
		return false, fmt.Errorf("cannot remove %s: %s", path, cause(err))
	}
	return removed, nil
}
