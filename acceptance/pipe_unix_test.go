//go:build unix

package acceptance

import (
	"bytes"
	"os/exec"
	"strings"
	"syscall"
	"testing"
)

// A reader that stops early (linearctl … | head) ends the run quietly, by
// SIGPIPE, with nothing on stderr: no "broken pipe" error to read past.
func TestClosedPipeEndsQuietly(t *testing.T) {
	t.Parallel()
	// ~20 MB of answers: more than any pipe holds, so a write meets the
	// closed end.
	cmd := exec.Command(bin, "serve", "--stdio")
	cmd.Dir = t.TempDir()
	cmd.Stdin = strings.NewReader(strings.Repeat(`{"id":1,"argv":["describe"]}`+"\n", 200))
	out, err := cmd.StdoutPipe()
	must(t, err)
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	must(t, cmd.Start())
	_, err = out.Read(make([]byte, 16))
	must(t, err)
	must(t, out.Close())
	_ = cmd.Wait()
	ws, _ := cmd.ProcessState.Sys().(syscall.WaitStatus)
	if !ws.Signaled() || ws.Signal() != syscall.SIGPIPE || stderr.Len() != 0 {
		t.Errorf("%v, stderr %q", cmd.ProcessState, stderr.String())
	}
}
