//go:build unix

package mutate

import (
	"os/exec"
	"strconv"
	"syscall"
)

// killTree makes a timeout end the whole process group: go test runs the
// test binary as a child, and a test may start QEMU, all of which would
// otherwise keep running.
func killTree(cmd *exec.Cmd) {
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	cmd.Cancel = func() error {
		return syscall.Kill(-cmd.Process.Pid, syscall.SIGKILL)
	}
}

// reapTree kills whatever is left of the command's process group after it
// exited. A test that finishes but leaves a background process behind, or a
// mutant that breaks the runner's own timeout handling, must not leave
// processes running once its run is over.
func reapTree(cmd *exec.Cmd) {
	if cmd.Process != nil {
		syscall.Kill(-cmd.Process.Pid, syscall.SIGKILL)
	}
}

// limitFiles wraps a command so no file it or its children write can grow
// beyond max bytes. A mutant that loops while writing an image fails with
// "file too large" instead of filling the disk. POSIX counts ulimit -f in
// 512-byte blocks; a shell that counts 1024 doubles the cap, still bounded.
func limitFiles(name string, args []string, max int64) (string, []string) {
	blocks := strconv.FormatInt(max/512, 10)
	return "sh", append([]string{"-c", "ulimit -f " + blocks + " && exec \"$@\"", "sh", name}, args...)
}
