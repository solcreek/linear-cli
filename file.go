package main

import (
	"bytes"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"path"
	"strings"

	"github.com/spf13/cobra"
)

// maxDownload caps a file download; Linear's own upload limit is lower.
const maxDownload = 256 << 20

func addFile(root *cobra.Command) {
	file := &cobra.Command{
		Use:   "file",
		Short: "Download files attached to issues and comments",
		Args:  usageArgs(cobra.NoArgs),
		RunE:  func(cmd *cobra.Command, _ []string) error { return cmd.Help() },
	}
	download := &cobra.Command{
		Use:   "download URL",
		Short: "Download a file from uploads.linear.app into the current directory",
		Long: "Download a file from uploads.linear.app (an image or attachment URL in an issue\n" +
			"or comment) with the API key, into the current directory.\n" +
			"--output defaults to the URL's last path segment. An existing file with the same\n" +
			"content is changed: false; with different content the command fails.\n" +
			"--dry-run downloads to memory and reports what --yes would write.\n" +
			"Changes things: pass --dry-run to preview, --yes to apply.",
		Example: "  linearctl file download https://uploads.linear.app/8f…/2c…/a1… --output shot.png --yes",
		Args:    usageArgs(cobra.ExactArgs(1)),
		RunE: func(cmd *cobra.Command, args []string) error {
			dryRun, _ := change(cmd)
			u, err := uploadURL(args[0])
			if err != nil {
				return err
			}
			name, _ := cmd.Flags().GetString("output")
			if name == "" {
				name = path.Base(u.Path)
			}
			if _, err := plain("--output", name); err != nil {
				return err
			}
			cwd, err := os.Getwd()
			if err != nil {
				return err
			}
			target, err := inside(cwd, name)
			if err != nil {
				return err
			}
			cl, err := newClient()
			if err != nil {
				return err
			}
			data, kind, err := cl.download(u)
			if err != nil {
				return err
			}
			changed := true
			switch old, err := os.ReadFile(target); {
			case err == nil && bytes.Equal(old, data):
				changed = false
			case err == nil:
				return fmt.Errorf("%s exists with other content; pass another --output", target)
			case !errors.Is(err, os.ErrNotExist):
				return fmt.Errorf("cannot read %s: %s", target, cause(err))
			}
			if changed && !dryRun {
				if err := createFile(target, data); err != nil {
					return err
				}
			}
			verb := map[bool]string{true: "downloaded", false: "already there"}[changed]
			if changed && dryRun {
				verb = "would write"
			}
			return output(cmd, map[string]any{"changed": changed, "dryRun": dryRun, "path": target, "bytes": len(data), "contentType": kind},
				fmt.Sprintf("%s %s (%d bytes)", verb, target, len(data)))
		},
	}
	download.Flags().String("output", "", "write to `PATH` inside the current directory")
	mutating(download)
	file.AddCommand(download)
	root.AddCommand(file)
}

// uploadURL is URL if it is a file on Linear's file host, the only place the
// API key is sent outside the API (rubric A9).
func uploadURL(s string) (*url.URL, error) {
	if _, err := plain("URL", s); err != nil {
		return nil, err
	}
	origin, _ := url.Parse(uploadsOrigin())
	u, err := url.Parse(s)
	if err != nil || u.Scheme != origin.Scheme || u.Host != origin.Host || u.User != nil || strings.Trim(u.Path, "/") == "" {
		return nil, usageError{fmt.Errorf("URL must be a file on %s; got %q", uploadsOrigin(), s)}
	}
	return u, nil
}

// download fetches u with the API key.
func (c *client) download(u *url.URL) ([]byte, string, error) {
	req, err := http.NewRequest(http.MethodGet, u.String(), nil)
	if err != nil {
		return nil, "", err
	}
	req.Header.Set("Authorization", c.key)
	req.Header.Set("User-Agent", "linearctl/"+version)
	res, err := c.do(req)
	if err != nil {
		return nil, "", err
	}
	defer res.Body.Close()
	switch {
	case res.StatusCode == http.StatusUnauthorized:
		return nil, "", authError{errors.New("Linear rejected the API key; pipe a new one to 'linearctl auth login --yes'")}
	case res.StatusCode == http.StatusNotFound || res.StatusCode == http.StatusForbidden:
		return nil, "", fmt.Errorf("not found: %s (or this API key cannot see it)", u)
	case res.StatusCode != http.StatusOK:
		return nil, "", fmt.Errorf("%s answered HTTP %d", u.Host, res.StatusCode)
	}
	data, err := io.ReadAll(io.LimitReader(res.Body, maxDownload+1))
	if err != nil {
		return nil, "", fmt.Errorf("reading %s: %s", u, cause(err))
	}
	if len(data) > maxDownload {
		return nil, "", fmt.Errorf("%s is larger than %d MiB", u, maxDownload>>20)
	}
	return data, res.Header.Get("Content-Type"), nil
}

// createFile writes a new file, never through a link or over one that
// appeared since the check (conventions: Paths); a failed write removes it.
func createFile(target string, data []byte) error {
	f, err := os.OpenFile(target, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o644)
	if err != nil {
		return fmt.Errorf("cannot create %s: %s", target, cause(err))
	}
	ours.Store(target, true)
	defer ours.Delete(target)
	_, err = f.Write(data)
	if cerr := f.Close(); err == nil {
		err = cerr
	}
	if err != nil {
		os.Remove(target)
		return fmt.Errorf("cannot write %s: %s", target, cause(err))
	}
	return nil
}
