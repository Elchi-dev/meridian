//go:build unix

package mutate

import (
	"os/exec"
	"syscall"
)

// killTree makes a timeout end the whole process group: go test runs the
// test binary as a child, which would otherwise keep running.
func killTree(cmd *exec.Cmd) {
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	cmd.Cancel = func() error {
		return syscall.Kill(-cmd.Process.Pid, syscall.SIGKILL)
	}
}
