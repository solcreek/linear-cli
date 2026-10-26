package main

import (
	"encoding/json"
	"errors"
	"fmt"
	"strconv"
	"strings"

	"github.com/spf13/cobra"
	"github.com/vektah/gqlparser/v2/ast"
	"github.com/vektah/gqlparser/v2/parser"
)

func addAPI(root *cobra.Command) {
	api := &cobra.Command{
		Use:   "api",
		Short: "Send your own GraphQL query to Linear",
		Args:  usageArgs(cobra.NoArgs),
		RunE:  func(cmd *cobra.Command, _ []string) error { return cmd.Help() },
	}
	query := &cobra.Command{
		Use:   "query",
		Short: "Send a GraphQL query (read-only) and print Linear's answer",
		Long: "Send a GraphQL query and print Linear's answer as JSON: data, errors, and the\n" +
			"query's complexity (Linear allows 10,000 points per query). The request is\n" +
			"{\"query\": \"…\", \"variables\": {…}, \"operationName\": \"…\"} from --json-input.\n" +
			"Mutations and subscriptions are refused: changes go through the typed commands,\n" +
			"which preview with --dry-run. Errors alongside data are exit 3, as problems.\n" +
			"Schema and examples: https://linear.app/developers/graphql",
		Example: "  echo '{\"query\":\"query { viewer { name } }\"}' | linearctl api query --json-input -\n" +
			"  linearctl api query --json-input req.json --signed-urls 300",
		Args: usageArgs(cobra.NoArgs),
		RunE: func(cmd *cobra.Command, _ []string) error {
			path, _ := cmd.Flags().GetString("json-input")
			if path == "" {
				return usageError{errors.New("pass the request with --json-input PATH (- for stdin)")}
			}
			data, err := readInput("--json-input", path)
			if err != nil {
				return err
			}
			var req struct {
				Query         string         `json:"query"`
				Variables     map[string]any `json:"variables"`
				OperationName string         `json:"operationName"`
			}
			if err := json.Unmarshal(data, &req); err != nil {
				return usageError{fmt.Errorf("--json-input is not a JSON request object: %s", err)}
			}
			if strings.TrimSpace(req.Query) == "" {
				return usageError{errors.New("--json-input has no query")}
			}
			doc, perr := parser.ParseQuery(&ast.Source{Input: req.Query})
			if perr != nil {
				return usageError{fmt.Errorf("the query does not parse: %s", perr.Error())}
			}
			for _, op := range doc.Operations {
				if op.Operation != ast.Query {
					return usageError{fmt.Errorf("api query sends queries only; this is a %s. Use the typed commands to change things", op.Operation)}
				}
			}
			cl, err := newClient()
			if err != nil {
				return err
			}
			cl.signedURLs = cmd.Flags().Lookup("signed-urls").Value.(*wholeNumber).n
			body := map[string]any{"query": req.Query, "variables": req.Variables}
			if req.OperationName != "" {
				body["operationName"] = req.OperationName
			}
			resp, err := cl.send(body)
			if err != nil {
				return err
			}
			hasData := len(resp.Data) > 0 && string(resp.Data) != "null"
			if len(resp.Errors) > 0 && !hasData {
				return linearFailure(resp.Errors)
			}
			out := map[string]any{"data": resp.Data, "complexity": nil}
			if n, err := strconv.ParseFloat(cl.header.Get("X-Complexity"), 64); err == nil {
				out["complexity"] = n
			}
			if len(resp.Errors) > 0 {
				problems := []map[string]string{}
				for _, e := range resp.Errors {
					where := "query"
					if len(e.Path) > 0 {
						parts := []string{}
						for _, p := range e.Path {
							parts = append(parts, fmt.Sprint(p))
						}
						where = strings.Join(parts, ".")
					}
					problems = append(problems, map[string]string{"input": where, "error": e.Message})
				}
				out["problems"] = problems
			}
			if err := printJSON(out); err != nil {
				return err
			}
			if len(resp.Errors) > 0 {
				return partialError{fmt.Errorf("%d errors", len(resp.Errors))}
			}
			return nil
		},
	}
	query.Flags().String("json-input", "", "read the request from `PATH` (- for stdin)")
	query.Flags().Var(&wholeNumber{}, "signed-urls", "sign file URLs for `SECONDS` so they open without the key")
	api.AddCommand(query)
	root.AddCommand(api)
}
