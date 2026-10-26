package main

import (
	"errors"
	"fmt"
	"io"
	"os"
	"strings"

	"github.com/spf13/cobra"
)

func addAuth(root *cobra.Command) {
	auth := &cobra.Command{
		Use:   "auth",
		Short: "Store, check or remove the Linear API key",
		Args:  usageArgs(cobra.NoArgs),
		RunE:  func(cmd *cobra.Command, _ []string) error { return cmd.Help() },
	}

	login := &cobra.Command{
		Use:   "login",
		Short: "Check an API key read from stdin and store it",
		Long: "Check an API key read from stdin with Linear, then store it: in the macOS Keychain,\n" +
			"or in credentials.json under the config directory elsewhere and whenever\n" +
			"LINEARCTL_HOME is set. LINEAR_API_KEY, when set, wins over a stored key.\n" +
			"Create a key at https://linear.app/settings/account/security.\n" +
			"Changes things: pass --dry-run to check the key only, --yes to store it.",
		Example: "  pbpaste | linearctl auth login --yes\n  linearctl auth login --dry-run --json < key.txt",
		Args:    usageArgs(cobra.NoArgs),
		RunE: func(cmd *cobra.Command, _ []string) error {
			dryRun, _ := change(cmd)
			key, err := secretFromStdin()
			if err != nil {
				return err
			}
			c := newClientWith(key, "stdin")
			var who viewer
			if err := c.query(qViewer, nil, &who); err != nil {
				return err
			}
			stored, source, err := storedKey()
			if err != nil {
				return err
			}
			changed := stored != key
			if changed && !dryRun {
				if err := storeKey(key); err != nil {
					return err
				}
			}
			verb := map[bool]string{true: "stored", false: "already stored"}[changed]
			if changed && dryRun {
				verb = "would store"
			}
			if os.Getenv("LINEAR_API_KEY") != "" {
				fmt.Fprintln(os.Stderr, "linearctl: note: LINEAR_API_KEY is set and wins over the stored key")
			}
			return output(cmd, map[string]any{"changed": changed, "dryRun": dryRun, "store": source,
				"user": who.Viewer, "organization": who.Organization},
				fmt.Sprintf("%s the key for %s (%s) in the %s", verb, printable(who.Viewer.Name), printable(who.Organization.Name), source))
		},
	}
	mutating(login)
	login.Annotations["stdin"] = "true"

	status := &cobra.Command{
		Use:     "status",
		Short:   "Show whose key is in use, where it comes from, and the rate limit left",
		Example: "  linearctl auth status --json",
		Args:    usageArgs(cobra.NoArgs),
		RunE: func(cmd *cobra.Command, _ []string) error {
			c, err := newClient()
			if err != nil {
				return err
			}
			var who viewer
			if err := c.query(qViewer, nil, &who); err != nil {
				return err
			}
			limits := c.rateLimit()
			text := fmt.Sprintf("%s <%s> in %s; key from %s", printable(who.Viewer.Name), printable(who.Viewer.Email),
				printable(who.Organization.Name), c.source)
			if r, ok := limits["requests"].(map[string]any); ok {
				text += fmt.Sprintf("; %v requests left this hour", r["remaining"])
			}
			return output(cmd, map[string]any{"source": c.source, "user": who.Viewer, "organization": who.Organization,
				"rateLimit": limits}, text)
		},
	}

	logout := &cobra.Command{
		Use:     "logout",
		Short:   "Remove the stored API key",
		Long:    "Remove the stored API key. LINEAR_API_KEY, if set, still applies.\nChanges things: pass --dry-run to preview, --yes to apply.",
		Example: "  linearctl auth logout --yes",
		Args:    usageArgs(cobra.NoArgs),
		RunE: func(cmd *cobra.Command, _ []string) error {
			dryRun, _ := change(cmd)
			stored, source, err := storedKey()
			if err != nil {
				return err
			}
			changed := stored != ""
			if changed && !dryRun {
				if changed, err = forgetKey(); err != nil {
					return err
				}
			}
			verb := map[bool]string{true: "removed", false: "no key stored"}[changed]
			if changed && dryRun {
				verb = "would remove"
			}
			if os.Getenv("LINEAR_API_KEY") != "" {
				fmt.Fprintln(os.Stderr, "linearctl: note: LINEAR_API_KEY is still set")
			}
			return output(cmd, map[string]any{"changed": changed, "dryRun": dryRun, "store": source},
				fmt.Sprintf("%s (%s)", verb, source))
		},
	}
	mutating(logout)

	auth.AddCommand(login, status, logout)
	root.AddCommand(auth)
}

// secretFromStdin reads a key piped on stdin (conventions: Secrets): never
// from a terminal, one trailing newline dropped, never empty. Errors name the
// input, never its content.
func secretFromStdin() (string, error) {
	if isTerminal(os.Stdin) {
		return "", usageError{errors.New("pipe the API key on stdin (e.g. pbpaste | linearctl auth login --yes); stdin is a terminal")}
	}
	data, err := io.ReadAll(io.LimitReader(os.Stdin, 4096))
	if err != nil {
		return "", fmt.Errorf("cannot read stdin: %s", cause(err))
	}
	key := strings.TrimSuffix(strings.TrimSuffix(string(data), "\n"), "\r")
	if key == "" {
		return "", usageError{errors.New("stdin is empty; pipe the API key")}
	}
	if !keyPattern.MatchString(key) {
		return "", usageError{errors.New("stdin does not look like a Linear API key (letters, digits, _ and -, 16 to 256 of them)")}
	}
	return key, nil
}
