package main

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strings"

	"github.com/spf13/cobra"
)

// configKeys are the settings config set takes, each with its check.
var configKeys = map[string]func(string) error{
	"default-team": func(v string) error {
		if !teamPattern.MatchString(v) {
			return usageError{fmt.Errorf("default-team must be a team key like ENG; got %q", v)}
		}
		return nil
	},
}

func addConfig(root *cobra.Command) {
	config := &cobra.Command{
		Use:   "config",
		Short: "Show or change settings",
		Args:  usageArgs(cobra.NoArgs),
		RunE:  func(cmd *cobra.Command, _ []string) error { return cmd.Help() },
	}
	keys := slices.Sorted(func(yield func(string) bool) {
		for k := range configKeys {
			if !yield(k) {
				return
			}
		}
	})

	show := &cobra.Command{
		Use:     "show",
		Short:   "Show the settings and the file they live in",
		Example: "  linearctl config show --json",
		Args:    usageArgs(cobra.NoArgs),
		RunE: func(cmd *cobra.Command, _ []string) error {
			cfg, err := readConfig()
			if err != nil {
				return err
			}
			path := ""
			if dir := home("config"); dir != "" {
				path = filepath.Join(dir, "config.json")
			}
			lines := []string{}
			for _, k := range keys {
				lines = append(lines, fmt.Sprintf("%s = %s", k, orDash(printable(cfg[k]))))
			}
			return output(cmd, map[string]any{"path": path, "settings": cfg}, strings.Join(lines, "\n"))
		},
	}

	set := &cobra.Command{
		Use:   "set KEY VALUE",
		Short: "Change a setting",
		Long: "Change a setting. Keys: " + strings.Join(keys, ", ") + ".\n" +
			"default-team is the team --team falls back to after LINEARCTL_TEAM.\n" +
			"Changes things: pass --dry-run to preview, --yes to apply.",
		Example: "  linearctl config set default-team ENG --yes",
		Args:    usageArgs(cobra.ExactArgs(2)),
		RunE: func(cmd *cobra.Command, args []string) error {
			dryRun, _ := change(cmd)
			key, value := args[0], args[1]
			check, ok := configKeys[key]
			if !ok {
				return usageError{fmt.Errorf("no setting %q; keys: %s", key, strings.Join(keys, ", "))}
			}
			if err := check(value); err != nil {
				return err
			}
			if key == "default-team" {
				value = strings.ToUpper(value)
			}
			dir := home("config")
			if dir == "" {
				return errors.New("no home directory; set LINEARCTL_HOME or XDG_CONFIG_HOME")
			}
			changed := false
			edit := func() error {
				cfg, err := readConfig()
				if err != nil {
					return err
				}
				changed = cfg[key] != value
				if !changed || dryRun {
					return nil
				}
				cfg[key] = value
				data, _ := json.MarshalIndent(cfg, "", "  ")
				return replaceFile(dir, "config.json", append(data, '\n'))
			}
			var err error
			if dryRun {
				err = edit()
			} else {
				if err = os.MkdirAll(dir, 0o700); err != nil {
					return fmt.Errorf("cannot create %s: %s", dir, cause(err))
				}
				err = withLock(dir, "config.json", edit)
			}
			if err != nil {
				return err
			}
			verb := map[bool]string{true: "set", false: "already"}[changed]
			if changed && dryRun {
				verb = "would set"
			}
			return output(cmd, map[string]any{"changed": changed, "dryRun": dryRun, "key": key, "value": value},
				fmt.Sprintf("%s %s = %s", verb, key, value))
		},
	}
	mutating(set)

	config.AddCommand(show, set)
	root.AddCommand(config)
}
