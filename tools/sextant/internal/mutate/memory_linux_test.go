package mutate

import (
	"bufio"
	"bytes"
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"
)

// TestMain doubles as the process the memory tests watch: started again
// with MUTATE_HOLD set to a number of MiB, the test binary holds that much,
// says so, and waits to be killed instead of running any test.
func TestMain(m *testing.M) {
	if mib, err := strconv.Atoi(os.Getenv("MUTATE_HOLD")); err == nil {
		hold(mib)
	}
	os.Exit(m.Run())
}

var holding []byte

func hold(mib int) {
	holding = bytes.Repeat([]byte{1}, mib<<20)
	fmt.Println("holding")
	time.Sleep(time.Hour)
}

// holder starts this test binary again, holding mib MiB with dir as its
// temporary folder, and returns its process id once the memory is held.
func holder(t *testing.T, dir string, mib int) int {
	t.Helper()
	cmd := exec.Command(os.Args[0], "-test.run=^$")
	cmd.Env = append(os.Environ(), "TMPDIR="+dir, "MUTATE_HOLD="+strconv.Itoa(mib))
	out, err := cmd.StdoutPipe()
	if err != nil {
		t.Fatal(err)
	}
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		cmd.Process.Kill()
		cmd.Wait()
	})
	if line, err := bufio.NewReader(out).ReadString('\n'); line != "holding\n" {
		t.Fatalf("the holder said %q: %v", line, err)
	}
	return cmd.Process.Pid
}

// resident is what /proc/<pid>/status says the process holds, in bytes.
func resident(t *testing.T, pid int) int64 {
	t.Helper()
	status, err := os.ReadFile(filepath.Join("/proc", strconv.Itoa(pid), "status"))
	if err != nil {
		t.Fatal(err)
	}
	for _, line := range strings.Split(string(status), "\n") {
		if kb, ok := strings.CutPrefix(line, "VmRSS:"); ok {
			n, err := strconv.ParseInt(strings.TrimSpace(strings.TrimSuffix(kb, "kB")), 10, 64)
			if err != nil {
				t.Fatal(err)
			}
			return n << 10
		}
	}
	t.Fatalf("no VmRSS for %d", pid)
	return 0
}

func TestHeldCountsTheRunsProcessesOnly(t *testing.T) {
	base := t.TempDir()
	run := filepath.Join(base, "run-1")
	in := holder(t, run, 64)
	nested := holder(t, filepath.Join(run, "inner", "run-7"), 32)
	holder(t, filepath.Join(base, "run-12"), 128)
	// Each holder is a whole test binary, which holds memory of its own on
	// top of what it was asked to, much more of it under the race detector,
	// so the run is weighed against what its two processes say they hold.
	want := resident(t, in) + resident(t, nested)
	got := held(run)
	if got < 96<<20 || got < want-8<<20 || got > want+8<<20 {
		t.Fatalf("the run holds %d MiB, want what its two processes hold, %d MiB", got>>20, want>>20)
	}
	if got := held(filepath.Join(base, "run-3")); got != 0 {
		t.Fatalf("a run without processes holds %d bytes", got)
	}
}

// holdingWorker is a worker whose runs start this test binary holding mib
// MiB.
func holdingWorker(t *testing.T, mib int) (*worker, []string) {
	t.Setenv("MUTATE_HOLD", strconv.Itoa(mib))
	w := &worker{dir: t.TempDir(), target: t.TempDir(), tmp: t.TempDir(), maxFile: 1 << 30}
	return w, []string{os.Args[0], "-test.run=^$"}
}

func TestARunHoldingMoreThanItsLimitIsStopped(t *testing.T) {
	w, command := holdingWorker(t, 64)
	started := time.Now()
	run := w.run(context.Background(), w.dir, command, limit{time: time.Minute, memory: 16 << 20})
	if !run.overMemory || run.err == nil || run.timedOut {
		t.Fatalf("over %v, error %v, timed out %v; the run should be stopped for its memory", run.overMemory, run.err, run.timedOut)
	}
	if time.Since(started) > 30*time.Second {
		t.Fatalf("stopping took %v; the memory is held within a second", time.Since(started))
	}
}

func TestARunWithoutAMemoryLimitIsMeasuredOnly(t *testing.T) {
	w, command := holdingWorker(t, 64)
	run := w.run(context.Background(), w.dir, command, limit{time: 3 * time.Second})
	if run.overMemory || !run.timedOut {
		t.Fatalf("over %v, timed out %v; only the timeout should end this run", run.overMemory, run.timedOut)
	}
	if run.peak < 64<<20 || run.peak > 1<<30 {
		t.Fatalf("peak %d MiB, want the 64 held and what the binary needs itself, far below a GiB", run.peak>>20)
	}
}

// Fill keeps a MiB for every doubling from 1 up to n, a little slowly, and
// the *= to /= mutant keeps going for ever. The test caps its own address
// space at 2 GiB, so a runner that failed to stop it ends it there.
const fillSource = `package m

import (
	"bytes"
	"time"
)

func Fill(n int) [][]byte {
	var kept [][]byte
	for i := 1; i < n; i *= 2 {
		kept = append(kept, bytes.Repeat([]byte{1}, 1<<20))
		time.Sleep(5 * time.Millisecond)
	}
	return kept
}
`

const fillTests = `package m

import (
	"syscall"
	"testing"
)

func TestFill(t *testing.T) {
	if err := syscall.Setrlimit(syscall.RLIMIT_AS, &syscall.Rlimit{Cur: 2 << 30, Max: 2 << 30}); err != nil {
		t.Fatal(err)
	}
	if len(Fill(8)) != 3 {
		t.Fatal("Fill is wrong")
	}
}
`

func TestAMutantThatKeepsAllocatingRunsOutOfMemory(t *testing.T) {
	root := writeTree(t, map[string]string{"go.mod": goModule, "a.go": fillSource, "a_test.go": fillTests})
	ms, err := GoMutants("a.go", []byte(fillSource))
	if err != nil {
		t.Fatal(err)
	}
	var picked []Mutant
	for _, m := range ms {
		if m.Desc == "*= to /=" || m.Desc == "< to >=" {
			picked = append(picked, m)
		}
	}
	if len(picked) != 2 {
		t.Fatalf("found %d of the two mutants", len(picked))
	}
	r := &Runner{Root: root, Jobs: 1, Scratch: t.TempDir(), Timeout: time.Minute, MaxMemory: 384 << 20}
	results, err := r.Run(context.Background(), picked, map[string]Target{"a.go": {Component: ".", Kind: "go"}})
	if err != nil {
		t.Fatal(err)
	}
	got := outcomes(results)
	if got["10 *= to /="] != OutOfMemory || got["10 < to >="] != Killed {
		t.Fatalf("outcomes %v; the endless one should run out of memory and the other be killed, under the same limit", got)
	}
}
