//go:build unix

package mutate

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"
)

// A test that writes a temporary file and records its process id, then
// runs Steps, which the *= to /= mutant turns into an endless loop.
const leavesTraces = `package m

import (
	"os"
	"strconv"
	"testing"
)

func TestSteps(t *testing.T) {
	f, err := os.CreateTemp("", "leftover-")
	if err != nil {
		t.Fatal(err)
	}
	f.WriteString("written before the endless loop")
	f.Close()
	os.WriteFile(os.Getenv("MUTATE_PID_FILE"), []byte(strconv.Itoa(os.Getpid())), 0o644)
	if Steps(8) != 3 {
		t.Fatal("Steps is wrong")
	}
}
`

func loopMutant(t *testing.T) Mutant {
	t.Helper()
	ms, err := GoMutants("a.go", []byte(goSource))
	if err != nil {
		t.Fatal(err)
	}
	for _, m := range ms {
		if m.Desc == "*= to /=" {
			return m
		}
	}
	t.Fatal("no *= mutant")
	return Mutant{}
}

func filesNamed(t *testing.T, root, prefix string) []string {
	t.Helper()
	var found []string
	filepath.WalkDir(root, func(p string, d os.DirEntry, err error) error {
		if err == nil && strings.HasPrefix(d.Name(), prefix) {
			found = append(found, p)
		}
		return nil
	})
	return found
}

// Go's own cleanup of t.TempDir never runs in a test that is killed. The
// runner's per-run folder and the process group kill are what keep a
// timed-out mutant from leaving files and processes behind.
func TestATimedOutMutantLeavesNothingBehind(t *testing.T) {
	root := writeTree(t, map[string]string{"go.mod": goModule, "a.go": goSource, "a_test.go": leavesTraces})
	pidFile := filepath.Join(t.TempDir(), "pid")
	t.Setenv("MUTATE_PID_FILE", pidFile)
	scratch := t.TempDir()
	var during []string
	r := &Runner{Root: root, Jobs: 1, Scratch: scratch, Timeout: 3 * time.Second, Done: func(Result) {
		during = filesNamed(t, scratch, "leftover-")
	}}
	results, err := r.Run(context.Background(), []Mutant{loopMutant(t)}, map[string]Target{"a.go": {Component: ".", Kind: "go"}})
	if err != nil {
		t.Fatal(err)
	}
	if results[0].Outcome != TimedOut {
		t.Fatalf("outcome %v, want a timeout", results[0].Outcome)
	}
	if len(during) != 0 {
		t.Fatalf("temporary files of the killed run were still there after it: %v", during)
	}
	raw, err := os.ReadFile(pidFile)
	if err != nil {
		t.Fatal(err)
	}
	pid, err := strconv.Atoi(string(raw))
	if err != nil {
		t.Fatal(err)
	}
	gone := false
	for range 50 {
		if errors.Is(syscall.Kill(pid, 0), syscall.ESRCH) {
			gone = true
			break
		}
		time.Sleep(100 * time.Millisecond)
	}
	if !gone {
		syscall.Kill(pid, syscall.SIGKILL)
		t.Fatalf("the test binary %d outlived its timeout; the whole process group must be killed", pid)
	}
}

func TestEachRunHasItsOwnTemporaryFolder(t *testing.T) {
	root := writeTree(t, map[string]string{
		"go.mod":    goModule,
		"a.go":      goSource,
		"a_test.go": goTests + "\nfunc TestTempDir(t *testing.T) {\n\tif d := os.TempDir(); !strings.Contains(d, \"sextant-mutate-\") || !strings.Contains(d, \"run-\") {\n\t\tt.Fatalf(\"temp dir %s is not the run's own\", d)\n\t}\n}\n",
	})
	if err := os.WriteFile(filepath.Join(root, "a_test.go"), []byte(strings.Replace(readFile(t, root, "a_test.go"), "import \"testing\"", "import (\n\t\"os\"\n\t\"strings\"\n\t\"testing\"\n)", 1)), 0o644); err != nil {
		t.Fatal(err)
	}
	scratch := t.TempDir()
	var leftovers []string
	r := &Runner{Root: root, Jobs: 1, Scratch: scratch, Timeout: 30 * time.Second, Done: func(Result) {
		leftovers = filesNamed(t, scratch, "run-")
	}}
	ms, _ := GoMutants("a.go", []byte(goSource))
	if _, err := r.Run(context.Background(), ms[:1], map[string]Target{"a.go": {Component: ".", Kind: "go"}}); err != nil {
		t.Fatal(err)
	}
	if len(leftovers) != 0 {
		t.Fatalf("run folders left after the run: %v", leftovers)
	}
}

func readFile(t *testing.T, root, name string) string {
	t.Helper()
	b, err := os.ReadFile(filepath.Join(root, name))
	if err != nil {
		t.Fatal(err)
	}
	return string(b)
}

const writesThreeMiB = `package m

import (
	"bytes"
	"os"
	"path/filepath"
	"testing"
)

func TestBigFile(t *testing.T) {
	if err := os.WriteFile(filepath.Join(t.TempDir(), "big"), bytes.Repeat([]byte{1}, 3<<20), 0o644); err != nil {
		t.Fatal(err)
	}
}
`

func TestFilesAreCapped(t *testing.T) {
	root := writeTree(t, map[string]string{"go.mod": goModule, "a.go": goSource, "a_test.go": writesThreeMiB})
	ms, _ := GoMutants("a.go", []byte(goSource))
	targets := map[string]Target{"a.go": {Component: ".", Kind: "go"}}
	_, err := (&Runner{Root: root, Jobs: 1, Scratch: t.TempDir(), MaxFileSize: 1 << 20}).Run(context.Background(), ms[:1], targets)
	if err == nil || !strings.Contains(err.Error(), "fail before any mutation") || !strings.Contains(err.Error(), "file too large") {
		t.Fatalf("a 3 MiB file under a 1 MiB cap must fail: %v", err)
	}
	if _, err := (&Runner{Root: root, Jobs: 1, Scratch: t.TempDir(), MaxFileSize: 16 << 20}).Run(context.Background(), ms[:1], targets); err != nil {
		t.Fatalf("under a 16 MiB cap the same test passes: %v", err)
	}
}

func TestLimitFiles(t *testing.T) {
	name, args := limitFiles("go", []string{"test", "./x"}, 1<<30)
	if name != "sh" || strings.Join(args, "|") != "-c|ulimit -f 2097152 && exec \"$@\"|sh|go|test|./x" {
		t.Fatalf("%s %q", name, args)
	}
}

const leavesABackgroundProcess = `package m

import (
	"os"
	"os/exec"
	"strconv"
	"testing"
)

func TestStartsSleep(t *testing.T) {
	cmd := exec.Command("sleep", "120")
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	os.WriteFile(os.Getenv("MUTATE_PID_FILE"), []byte(strconv.Itoa(cmd.Process.Pid)), 0o644)
}
`

// A test run that passes but leaves a process behind: the runner ends it
// with the run, not only when a run times out.
func TestARunLeavesNoProcessesBehind(t *testing.T) {
	root := writeTree(t, map[string]string{"go.mod": goModule, "a.go": goSource, "a_test.go": leavesABackgroundProcess})
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
	t.Fatalf("sleep %d kept running after its run", pid)
}
