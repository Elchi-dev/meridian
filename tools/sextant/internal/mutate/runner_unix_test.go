//go:build unix

package mutate

import (
	"context"
	"errors"
	"os"
	"os/exec"
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

const reportsNiceness = `package m

import (
	"os"
	"os/exec"
	"strconv"
	"testing"
)

func TestNiceness(t *testing.T) {
	out, err := exec.Command("ps", "-o", "nice=", "-p", strconv.Itoa(os.Getpid())).Output()
	if err != nil {
		t.Fatal(err)
	}
	os.WriteFile(os.Getenv("MUTATE_NICE_FILE"), out, 0o644)
}
`

func niceness(t *testing.T, ps []byte) int {
	t.Helper()
	n, err := strconv.Atoi(strings.TrimSpace(string(ps)))
	if err != nil {
		t.Fatalf("ps printed %q: %v", ps, err)
	}
	return n
}

// Mutants run under nice, so a long mutation run leaves the machine usable.
func TestTestsRunAtALowerPriority(t *testing.T) {
	for _, tool := range []string{"nice", "ps"} {
		if _, err := exec.LookPath(tool); err != nil {
			t.Skipf("%s is not installed", tool)
		}
	}
	own, err := exec.Command("ps", "-o", "nice=", "-p", strconv.Itoa(os.Getpid())).Output()
	if err != nil {
		t.Fatal(err)
	}
	before := niceness(t, own)
	if before >= 19 {
		t.Skip("this test already runs at the lowest priority, so a lower one cannot be seen")
	}
	marker := filepath.Join(t.TempDir(), "nice")
	t.Setenv("MUTATE_NICE_FILE", marker)
	root := writeTree(t, map[string]string{"go.mod": goModule, "a.go": goSource, "a_test.go": reportsNiceness})
	ms, _ := GoMutants("a.go", []byte(goSource))
	if _, err := (&Runner{Root: root, Jobs: 1, Scratch: t.TempDir()}).Run(context.Background(), ms[:1], map[string]Target{"a.go": {Component: ".", Kind: "go"}}); err != nil {
		t.Fatal(err)
	}
	raw, err := os.ReadFile(marker)
	if err != nil {
		t.Fatal(err)
	}
	if got, want := niceness(t, raw), min(before+15, 19); got != want {
		t.Fatalf("the tests ran at nice %d, want %d", got, want)
	}
}

// nice is a convenience: where it is missing the tests run at normal
// priority instead of not at all.
func TestRunnerWorksWithoutNice(t *testing.T) {
	bin := t.TempDir()
	for _, tool := range []string{"go", "sh"} {
		p, err := exec.LookPath(tool)
		if err != nil {
			t.Fatal(err)
		}
		if err := os.Symlink(p, filepath.Join(bin, tool)); err != nil {
			t.Fatal(err)
		}
	}
	t.Setenv("PATH", bin)
	root := writeTree(t, map[string]string{"go.mod": goModule, "a.go": goSource, "a_test.go": goTests})
	ms, _ := GoMutants("a.go", []byte(goSource))
	var picked []Mutant
	for _, m := range ms {
		if m.Line == 4 && m.Desc == "< to >=" {
			picked = append(picked, m)
		}
	}
	results, err := (&Runner{Root: root, Jobs: 1, Scratch: t.TempDir()}).Run(context.Background(), picked, map[string]Target{"a.go": {Component: ".", Kind: "go"}})
	if err != nil || len(results) != 1 || results[0].Outcome != Killed {
		t.Fatalf("got %v %v", results, err)
	}
}

// A test whose child leaves the process group, and so survives the kill,
// still holds the output Cargo passed on to it.
const escapesTheGroup = `pub fn less(a: u8, b: u8) -> bool {
    a < b
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::CommandExt;

    #[test]
    fn hangs_when_broken() {
        if super::less(1, 2) {
            return;
        }
        let child = std::process::Command::new("sleep")
            .arg("60")
            .process_group(0)
            .spawn()
            .unwrap();
        std::fs::write(std::env::var("MUTATE_PID_FILE").unwrap(), child.id().to_string()).unwrap();
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }
}
`

func TestAnEscapedProcessDoesNotStallTheRun(t *testing.T) {
	if _, err := exec.LookPath("cargo"); err != nil {
		if os.Getenv("SEXTANT_REQUIRE_TOOLS") == "1" {
			t.Fatal("cargo is not installed")
		}
		t.Skip("cargo is not installed")
	}
	pidFile := filepath.Join(t.TempDir(), "pid")
	t.Setenv("MUTATE_PID_FILE", pidFile)
	t.Cleanup(func() {
		if raw, err := os.ReadFile(pidFile); err == nil {
			if pid, err := strconv.Atoi(string(raw)); err == nil {
				syscall.Kill(pid, syscall.SIGKILL)
			}
		}
	})
	root := writeTree(t, map[string]string{
		"k/Cargo.toml": "[package]\nname = \"k\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
		"k/src/lib.rs": escapesTheGroup,
	})
	ms, err := RustMutants("k/src/lib.rs", []byte(escapesTheGroup), func(line int) bool { return line > 4 })
	if err != nil {
		t.Fatal(err)
	}
	var picked []Mutant
	for _, m := range ms {
		if m.Desc == "body to { false }" {
			picked = append(picked, m)
		}
	}
	r := &Runner{Root: root, Jobs: 1, Scratch: t.TempDir(), Timeout: 4 * time.Second}
	started := time.Now()
	results, err := r.Run(context.Background(), picked, map[string]Target{"k/src/lib.rs": {Component: "k", Kind: "rust"}})
	if err != nil || len(results) != 1 || results[0].Outcome != TimedOut {
		t.Fatalf("got %v %v", results, err)
	}
	if _, err := os.Stat(pidFile); err != nil {
		t.Fatalf("the mutant never started the escaping process: %v", err)
	}
	if took := time.Since(started); took > 40*time.Second {
		t.Fatalf("the run took %v; it waited for the escaped process instead of giving up on its output", took)
	}
}
