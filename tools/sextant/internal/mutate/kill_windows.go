//go:build windows

package mutate

import (
	"os/exec"
	"strconv"
)

// killTree makes a timeout end the test command and every process it
// started, which on Windows takes taskkill's /T.
func killTree(cmd *exec.Cmd) {
	cmd.Cancel = func() error {
		return exec.Command("taskkill", "/T", "/F", "/PID", strconv.Itoa(cmd.Process.Pid)).Run()
	}
}
