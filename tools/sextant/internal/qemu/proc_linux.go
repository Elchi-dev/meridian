package qemu

import (
	"os/exec"
	"syscall"
)

// tieToParent makes the kernel kill QEMU when the process that started it
// dies, so a test killed by a timeout leaves no machine running behind it.
func tieToParent(cmd *exec.Cmd) {
	cmd.SysProcAttr = &syscall.SysProcAttr{Pdeathsig: syscall.SIGKILL}
}
