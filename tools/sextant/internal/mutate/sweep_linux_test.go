package mutate

import (
	"context"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"syscall"
	"testing"
	"time"
)

func startSleep(t *testing.T, tmpdir string) *exec.Cmd {
	t.Helper()
	cmd := exec.Command("sleep", "120")
	cmd.Env = append(os.Environ(), "TMPDIR="+tmpdir)
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		cmd.Process.Kill()
		cmd.Wait()
	})
	return cmd
}

// exited waits for cmd to end, which it only does when killed.
func exited(cmd *exec.Cmd) bool {
	done := make(chan struct{})
	go func() {
		cmd.Process.Wait()
		close(done)
	}()
	select {
	case <-done:
		return true
	case <-time.After(2 * time.Second):
		return false
	}
}

func TestSweepKillsTheRunsProcessesOnly(t *testing.T) {
	base := t.TempDir()
	run := filepath.Join(base, "run-1")
	inRun := startSleep(t, run)
	nested := startSleep(t, filepath.Join(run, "inner", "run-7"))
	sibling := startSleep(t, filepath.Join(base, "run-12"))
	sweep(run)
	if !exited(inRun) || !exited(nested) {
		t.Fatal("a process of the run is still there")
	}
	if exited(sibling) {
		t.Fatal("the sweep killed a process of another run")
	}
}

// A test that starts a process in a group of its own and passes. Killing
// the run's process group misses that process; the sweep does not.
const leavesAProcessInItsOwnGroup = `package m

import (
	"os"
	"os/exec"
	"strconv"
	"syscall"
	"testing"
)

func TestStartsSleep(t *testing.T) {
	cmd := exec.Command("sleep", "120")
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	os.WriteFile(os.Getenv("MUTATE_PID_FILE"), []byte(strconv.Itoa(cmd.Process.Pid)), 0o644)
}
`

func TestARunLeavesNoProcessesInOtherGroupsBehind(t *testing.T) {
	root := writeTree(t, map[string]string{"go.mod": goModule, "a.go": goSource, "a_test.go": leavesAProcessInItsOwnGroup})
	pidFile := filepath.Join(t.TempDir(), "pid")
	t.Setenv("MUTATE_PID_FILE", pidFile)
	ms, _ := GoMutants("a.go", []byte(goSource))
	r := &Runner{Root: root, Jobs: 1, Scratch: t.TempDir(), Timeout: 30 * time.Second}
	if _, err := r.Run(context.Background(), ms[:1], map[string]Target{"a.go": {Component: ".", Kind: "go"}}); err != nil {
		t.Fatal(err)
	}
	raw, err := os.ReadFile(pidFile)
	if err != nil {
		t.Fatal(err)
	}
	pid, _ := strconv.Atoi(string(raw))
	for range 50 {
		if errors.Is(syscall.Kill(pid, 0), syscall.ESRCH) {
			return
		}
		time.Sleep(100 * time.Millisecond)
	}
	syscall.Kill(pid, syscall.SIGKILL)
	t.Fatalf("sleep %d in its own process group kept running after its run", pid)
}
