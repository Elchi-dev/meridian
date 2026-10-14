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

// reapTree does nothing on Windows: once the command has exited, taskkill
// can no longer find its children by parent. A job object would; mutation
// runs happen on Linux, so that waits until Windows needs it.
func reapTree(*exec.Cmd) {}

// limitFiles leaves the command as it is. Windows has no per-process file
// size limit a child inherits; mutation runs happen on Linux in CI, and the
// per-run temporary folder is still removed after every command.
func limitFiles(name string, args []string, _ int64) (string, []string) {
	return name, args
}
