//go:build unix

package qemu

import (
	"strings"
	"syscall"
	"testing"
	"time"
)

// A monitor that stops answering ends the command with an error, rather
// than taking keys into a socket nothing empties.
func TestAMonitorThatStopsAnsweringIsReported(t *testing.T) {
	cmd, address := bareMachine(t)
	w := &scriptWatcher{address: address, patience: 3 * time.Second}
	t.Cleanup(func() {
		if w.monitor != nil {
			w.monitor.Close()
		}
	})
	if err := w.press("a"); err != nil {
		t.Fatalf("first key: %v", err)
	}
	if err := cmd.Process.Signal(syscall.SIGSTOP); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = cmd.Process.Signal(syscall.SIGCONT) })
	err := w.press("b")
	if err == nil || !strings.Contains(err.Error(), `did not answer "sendkey b"`) {
		t.Fatalf("got %v", err)
	}
}
