package main

import (
	"bufio"
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"slices"
	"strings"

	"github.com/spf13/cobra"
)

// servedKey is the API key serve read once at start, so a request does not
// read the Keychain again (~10 ms each; docs/performance.md).
var servedKey *struct{ key, source string }

func addServe(root *cobra.Command) {
	c := &cobra.Command{
		Use:   "serve",
		Short: "Answer linearctl invocations over stdin and stdout, one JSON line each",
		Long: "Answer linearctl invocations over stdin and stdout, for a program that makes many\n" +
			"calls (a TUI, a long agent session). One process serves them all: no start-up, no\n" +
			"new TLS connection, no Keychain read per call.\n\n" +
			"Each line on stdin is a request:   {\"id\": ANY, \"argv\": [\"issue\", \"list\", \"--json\"]}\n" +
			"Each line on stdout answers one:   {\"id\": ANY, \"exit\": N, \"result\": JSON|null,\n" +
			"                                    \"stdout\": TEXT|null, \"stderr\": TEXT}\n" +
			"argv is what follows linearctl on a command line; exit and the JSON in result are\n" +
			"exactly what that command line would give (result is null when it printed text,\n" +
			"which is then in stdout). Requests run one at a time, in order. A malformed line\n" +
			"is answered with exit 2. watch and serve are refused, and so is - as a file:\n" +
			"stdin carries requests. serve ends when stdin closes (exit 0).",
		Example: "  printf '{\"id\":1,\"argv\":[\"issue\",\"list\",\"--json\"]}\\n' | linearctl serve --stdio",
		Args:    usageArgs(cobra.NoArgs),
		RunE: func(cmd *cobra.Command, _ []string) error {
			if on, _ := cmd.Flags().GetBool("stdio"); !on {
				return usageError{errors.New("pass --stdio (the only transport so far)")}
			}
			return serveStdio(os.Stdin, stdout)
		},
	}
	c.Flags().Bool("stdio", false, "read requests on stdin, answer on stdout")
	root.AddCommand(c)
}

func serveStdio(in io.Reader, out io.Writer) error {
	if key, source, err := apiKey(); err == nil && key != "" {
		servedKey = &struct{ key, source string }{key, source}
	}
	lines := bufio.NewScanner(in)
	lines.Buffer(make([]byte, 64<<10), 16<<20)
	enc := json.NewEncoder(out)
	for lines.Scan() {
		if len(bytes.TrimSpace(lines.Bytes())) == 0 {
			continue
		}
		if err := enc.Encode(serveOne(lines.Bytes())); err != nil {
			return fmt.Errorf("cannot answer on stdout: %s", cause(err))
		}
	}
	if err := lines.Err(); err != nil {
		return fmt.Errorf("cannot read requests: %s", cause(err))
	}
	return nil
}

// serveOne runs one request line and returns its answer.
func serveOne(line []byte) map[string]any {
	var req struct {
		ID   any      `json:"id"`
		Argv []string `json:"argv"`
	}
	refuse := func(id any, msg string) map[string]any {
		return map[string]any{"id": id, "exit": exitUsage, "result": map[string]any{"error": msg, "usage": true},
			"stdout": nil, "stderr": "linearctl: " + msg + "\n"}
	}
	if err := json.Unmarshal(line, &req); err != nil {
		return refuse(nil, "a request is one JSON object: {\"id\": ..., \"argv\": [...]}")
	}
	switch {
	case len(req.Argv) == 0:
		return refuse(req.ID, "argv is empty")
	case req.Argv[0] == "serve" || req.Argv[0] == "watch":
		return refuse(req.ID, req.Argv[0]+" cannot run inside serve; run it as its own process")
	case slices.Contains(req.Argv, "-") || slices.ContainsFunc(req.Argv, func(a string) bool { return strings.HasSuffix(a, "=-") }):
		return refuse(req.ID, "- (stdin) is serve's own input; pass the text in argv, or a file")
	}
	var o, e bytes.Buffer
	stdout, stderr = &o, &e
	code := execute(req.Argv)
	stdout, stderr = os.Stdout, os.Stderr
	answer := map[string]any{"id": req.ID, "exit": code, "result": nil, "stdout": nil, "stderr": e.String()}
	var result any
	if json.Unmarshal(o.Bytes(), &result) == nil {
		answer["result"] = result
	} else if o.Len() > 0 {
		answer["stdout"] = o.String()
	}
	return answer
}
