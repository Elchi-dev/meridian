package mutate

import (
	"context"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/Elchi-dev/meridian/tools/sextant/internal/stats"
)

func writeTree(t *testing.T, files map[string]string) string {
	t.Helper()
	root := t.TempDir()
	for name, body := range files {
		p := filepath.Join(root, filepath.FromSlash(name))
		if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(p, []byte(body), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	return root
}

const goModule = "module m\n\ngo 1.27.0\n"

const goSource = `package m

func Less(a, b int) bool {
	return a < b
}

func Join(a, b string) string {
	return a + b
}

func Steps(n int) int {
	steps := 0
	for i := 1; i < n; i *= 2 {
		steps++
	}
	return steps
}
`

const goTests = `package m

import "testing"

func TestLess(t *testing.T) {
	if !Less(1, 2) || Less(2, 1) {
		t.Fatal("Less is wrong")
	}
}

func TestJoin(t *testing.T) {
	if Join("a", "b") != "ab" {
		t.Fatal("Join is wrong")
	}
}

func TestSteps(t *testing.T) {
	if Steps(8) != 3 {
		t.Fatal("Steps is wrong")
	}
}
`

func outcomes(results []Result) map[string]Outcome {
	out := map[string]Outcome{}
	for _, r := range results {
		out[itoa(r.Mutant.Line)+" "+r.Mutant.Desc] = r.Outcome
	}
	return out
}

// Each mutant of a small Go module is really built and tested. The module
// is chosen so that every outcome shows up once.
func TestRunnerOnAGoModule(t *testing.T) {
	root := writeTree(t, map[string]string{"go.mod": goModule, "a.go": goSource, "a_test.go": goTests})
	ms, err := GoMutants("a.go", []byte(goSource))
	if err != nil {
		t.Fatal(err)
	}
	var mu sync.Mutex
	done := 0
	r := &Runner{Root: root, Jobs: 2, Timeout: 5 * time.Second, Done: func(Result) {
		mu.Lock()
		done++
		mu.Unlock()
	}}
	results, err := r.Run(context.Background(), ms, map[string]Target{"a.go": {Component: ".", Kind: "go"}})
	if err != nil {
		t.Fatal(err)
	}
	got := outcomes(results)
	want := map[string]Outcome{
		"3 body to { return false }": Killed,
		"3 body to { return true }":  Killed,
		"4 < to <=":                  Survived,
		"4 < to >=":                  Killed,
		"8 + to -":                   Unviable,
		"13 < to <=":                 Killed,
		"13 < to >=":                 Killed,
		"13 *= to /=":                TimedOut,
	}
	for k, w := range want {
		if got[k] != w {
			t.Errorf("%s: %v, want %v", k, got[k], w)
		}
	}
	if len(results) != len(ms) || done != len(ms) {
		t.Fatalf("%d results and %d callbacks for %d mutants", len(results), done, len(ms))
	}
	for i := range results {
		if results[i].Mutant != ms[i] {
			t.Fatalf("result %d is for %v, want the mutants in order", i, results[i].Mutant)
		}
	}
	if src, _ := os.ReadFile(filepath.Join(root, "a.go")); string(src) != goSource {
		t.Fatal("the real working tree must never be changed")
	}
}

func TestRunnerRefusesTestsThatAlreadyFail(t *testing.T) {
	root := writeTree(t, map[string]string{
		"go.mod":    goModule,
		"a.go":      goSource,
		"a_test.go": "package m\n\nimport \"testing\"\n\nfunc TestBroken(t *testing.T) { t.Fatal(\"broken before any mutation\") }\n",
	})
	ms, _ := GoMutants("a.go", []byte(goSource))
	_, err := (&Runner{Root: root, Jobs: 1}).Run(context.Background(), ms[:1], map[string]Target{"a.go": {Component: ".", Kind: "go"}})
	if err == nil || !strings.Contains(err.Error(), "the tests of go:. fail before any mutation") || !strings.Contains(err.Error(), "broken before any mutation") {
		t.Fatalf("got %v", err)
	}
}

func TestRunnerNeedsATargetForEveryMutant(t *testing.T) {
	root := writeTree(t, map[string]string{"go.mod": goModule, "a.go": goSource})
	_, err := (&Runner{Root: root}).Run(context.Background(), []Mutant{{Path: "a.go"}}, nil)
	if err == nil || err.Error() != "a.go belongs to no component" {
		t.Fatalf("got %v", err)
	}
	results, err := (&Runner{Root: root}).Run(context.Background(), nil, nil)
	if results != nil || err != nil {
		t.Fatalf("no mutants: %v, %v", results, err)
	}
}

func TestRunnerReportsUnknownKinds(t *testing.T) {
	root := writeTree(t, map[string]string{"x/a.zig": "const a = 1;\n"})
	_, err := (&Runner{Root: root}).Run(context.Background(), []Mutant{{Path: "x/a.zig"}}, map[string]Target{"x/a.zig": {Component: "x", Kind: "zig"}})
	if err == nil || !strings.Contains(err.Error(), "fail before any mutation") {
		t.Fatalf("got %v", err)
	}
}

func TestRunnerReportsMissingFiles(t *testing.T) {
	root := t.TempDir()
	_, err := (&Runner{Root: root}).Run(context.Background(), []Mutant{{Path: "gone.go"}}, map[string]Target{"gone.go": {Component: ".", Kind: "go"}})
	if !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("got %v", err)
	}
}

func TestRunnerStopsWhenCancelled(t *testing.T) {
	root := writeTree(t, map[string]string{"go.mod": goModule, "a.go": goSource, "a_test.go": goTests})
	ms, _ := GoMutants("a.go", []byte(goSource))
	ctx, cancel := context.WithCancel(context.Background())
	r := &Runner{Root: root, Jobs: 1, Done: func(Result) { cancel() }}
	_, err := r.Run(ctx, ms, map[string]Target{"a.go": {Component: ".", Kind: "go"}})
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("got %v", err)
	}
}

const rustLib = `pub fn less(a: u8, b: u8) -> bool {
    a < b
}

pub fn join(a: &str, b: &str) -> String {
    a.to_owned() + b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn less_and_join() {
        assert!(less(1, 2) && !less(2, 1));
        assert_eq!(join("a", "b"), "ab");
    }
}
`

// The same for a Rust crate, built by Cargo in the worker's own target
// folder.
func TestRunnerOnARustCrate(t *testing.T) {
	if _, err := exec.LookPath("cargo"); err != nil {
		if os.Getenv("SEXTANT_REQUIRE_TOOLS") == "1" {
			t.Fatal("cargo is not installed")
		}
		t.Skip("cargo is not installed")
	}
	root := writeTree(t, map[string]string{
		"k/Cargo.toml": "[package]\nname = \"k\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
		"k/src/lib.rs": rustLib,
	})
	srcs, err := stats.Sources(os.DirFS(root))
	if err != nil || len(srcs) != 1 {
		t.Fatalf("%v %v", srcs, err)
	}
	ms, err := RustMutants("k/src/lib.rs", []byte(rustLib), srcs[0].IsTest)
	if err != nil {
		t.Fatal(err)
	}
	shared := filepath.Join(t.TempDir(), "shared-target")
	t.Setenv("CARGO_TARGET_DIR", shared)
	results, err := (&Runner{Root: root, Jobs: 1}).Run(context.Background(), ms, map[string]Target{"k/src/lib.rs": {Component: "k", Kind: "rust"}})
	if err != nil {
		t.Fatal(err)
	}
	got := outcomes(results)
	want := map[string]Outcome{
		"1 body to { false }": Killed,
		"1 body to { true }":  Killed,
		"2 < to <=":           Survived,
		"2 < to >=":           Killed,
		"6 + to -":            Unviable,
	}
	if len(got) != len(want) {
		t.Fatalf("mutants: %v", got)
	}
	for k, w := range want {
		if got[k] != w {
			t.Errorf("%s: %v, want %v", k, got[k], w)
		}
	}
	if _, err := os.Stat(shared); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("workers must build in their own target folders, not in CARGO_TARGET_DIR from the environment: %v", err)
	}
}

func TestCopyTreeLeavesOutHistoryAndBuildOutput(t *testing.T) {
	root := writeTree(t, map[string]string{
		"a.go":                  "a",
		"deep/nested/b.rs":      "b",
		"tests/meri/v.meri":     "v",
		"x/testdata/fuzz/seed":  "s",
		".git/HEAD":             "ref",
		"target/debug/big":      "big",
		"tools/out/image.img":   "img",
		"web/node_modules/m.js": "m",
		"outer/target-notes.md": "kept",
	})
	if err := os.Chmod(filepath.Join(root, "a.go"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink("a.go", filepath.Join(root, "link.go")); err != nil {
		t.Fatal(err)
	}
	dest := filepath.Join(t.TempDir(), "copy")
	if err := copyTree(root, dest); err != nil {
		t.Fatal(err)
	}
	var got []string
	filepath.WalkDir(dest, func(p string, d os.DirEntry, err error) error {
		if err == nil && !d.IsDir() {
			rel, _ := filepath.Rel(dest, p)
			got = append(got, filepath.ToSlash(rel))
		}
		return nil
	})
	sort.Strings(got)
	want := "a.go deep/nested/b.rs link.go outer/target-notes.md tests/meri/v.meri x/testdata/fuzz/seed"
	if strings.Join(got, " ") != want {
		t.Fatalf("copied %v\nwant   %s", got, want)
	}
	info, err := os.Stat(filepath.Join(dest, "a.go"))
	if err != nil || info.Mode().Perm() != 0o755 {
		t.Fatalf("mode not kept: %v %v", info, err)
	}
	if link, err := os.Readlink(filepath.Join(dest, "link.go")); err != nil || link != "a.go" {
		t.Fatalf("symlink: %q %v", link, err)
	}
	if b, _ := os.ReadFile(filepath.Join(dest, "deep/nested/b.rs")); string(b) != "b" {
		t.Fatalf("content: %q", b)
	}
}

func TestCopyTreeReportsAMissingSource(t *testing.T) {
	if err := copyTree(filepath.Join(t.TempDir(), "nope"), t.TempDir()); err == nil {
		t.Fatal("a missing source must be an error")
	}
}

func TestClassify(t *testing.T) {
	failed := errors.New("exit status 1")
	tests := []struct {
		kind string
		run  testRun
		want Outcome
	}{
		{"go", testRun{}, Survived},
		{"go", testRun{err: failed, output: []byte("--- FAIL: TestX\nFAIL")}, Killed},
		{"go", testRun{err: failed, output: []byte("FAIL\tm [build failed]")}, Unviable},
		{"go", testRun{err: failed, output: []byte("FAIL\tm [setup failed]")}, Unviable},
		{"go", testRun{err: failed, output: []byte("panic: test timed out after 3s")}, TimedOut},
		{"go", testRun{err: failed, timedOut: true}, TimedOut},
		{"rust", testRun{err: failed, output: []byte("error: could not compile `k` (lib)")}, Unviable},
		{"rust", testRun{err: failed, output: []byte("test result: FAILED")}, Killed},
		{"rust", testRun{err: failed, output: []byte("[build failed]")}, Killed},
		{"go", testRun{err: failed, output: []byte("error: could not compile")}, Killed},
		{"rust", testRun{}, Survived},
	}
	for i, tt := range tests {
		if got := classify(tt.kind, tt.run); got != tt.want {
			t.Errorf("case %d: %v, want %v", i, got, tt.want)
		}
	}
}

func TestOutcomeNames(t *testing.T) {
	names := map[Outcome]string{Killed: "killed", Survived: "survived", Unviable: "unviable", TimedOut: "timed out", 0: "Outcome(0)", 9: "Outcome(9)"}
	for o, want := range names {
		if o.String() != want {
			t.Errorf("%d: %q, want %q", int(o), o.String(), want)
		}
	}
}

func TestTestKey(t *testing.T) {
	if k := testKey("tools/x/internal/a/b.go", Target{Component: "tools/x", Kind: "go"}); k != "go:tools/x/internal/a" {
		t.Errorf("go: %s", k)
	}
	if k := testKey("lib/uefi/src/raw/x.rs", Target{Component: "lib/uefi", Kind: "rust"}); k != "rust:lib/uefi" {
		t.Errorf("rust: %s", k)
	}
}

func TestTail(t *testing.T) {
	if got := tail([]byte("a\nb\nc\n"), 2); got != "b\nc" {
		t.Errorf("got %q", got)
	}
	if got := tail([]byte("a\nb"), 5); got != "a\nb" {
		t.Errorf("got %q", got)
	}
	if got := tail(nil, 3); got != "" {
		t.Errorf("got %q", got)
	}
}

func TestFirstOf(t *testing.T) {
	a, b := errors.New("a"), errors.New("b")
	if firstOf(nil, b) != b || firstOf(a, b) != a || firstOf(nil, nil) != nil {
		t.Fatal("firstOf keeps the first error")
	}
}

// The workers' copies live in Scratch while the run lasts; counting them
// shows how many workers really ran.
func TestRunnerStartsOneCopyPerJob(t *testing.T) {
	root := writeTree(t, map[string]string{"go.mod": goModule, "a.go": goSource, "a_test.go": goTests})
	ms, _ := GoMutants("a.go", []byte(goSource))
	for jobs, want := range map[int]int{0: 1, 1: 1, 3: 3} {
		scratch := t.TempDir()
		var mu sync.Mutex
		seen := 0
		r := &Runner{Root: root, Jobs: jobs, Scratch: scratch, Timeout: 5 * time.Second, Done: func(Result) {
			trees, _ := filepath.Glob(filepath.Join(scratch, "sextant-mutate-*", "tree-*"))
			mu.Lock()
			seen = max(seen, len(trees))
			mu.Unlock()
		}}
		if _, err := r.Run(context.Background(), ms[:2], map[string]Target{"a.go": {Component: ".", Kind: "go"}}); err != nil {
			t.Fatal(err)
		}
		if seen != want {
			t.Errorf("Jobs %d: %d worker copies, want %d", jobs, seen, want)
		}
		if left, _ := filepath.Glob(filepath.Join(scratch, "*")); len(left) != 0 {
			t.Errorf("Jobs %d: copies left behind: %v", jobs, left)
		}
	}
}

// A worker must put the original back after each mutant: otherwise the
// next mutant, in another file, would be tested on top of the last one.
func TestRunnerRestoresEachFileBeforeTheNextMutant(t *testing.T) {
	other := "package m\n\nfunc Double(n int) int {\n\treturn n * 2\n}\n"
	root := writeTree(t, map[string]string{
		"go.mod":    goModule,
		"a.go":      goSource,
		"b.go":      other,
		"a_test.go": goTests + "\nfunc TestDouble(t *testing.T) {\n\tif Double(0) != 0 {\n\t\tt.Fatal(\"Double is wrong\")\n\t}\n}\n",
	})
	ma, _ := GoMutants("a.go", []byte(goSource))
	mb, _ := GoMutants("b.go", []byte(other))
	var steps Mutant
	for _, m := range ma {
		if m.Desc == "*= to /=" {
			steps = m
		}
	}
	targets := map[string]Target{"a.go": {Component: ".", Kind: "go"}, "b.go": {Component: ".", Kind: "go"}}
	results, err := (&Runner{Root: root, Jobs: 1, Timeout: 5 * time.Second}).Run(context.Background(), []Mutant{steps, mb[0]}, targets)
	if err != nil {
		t.Fatal(err)
	}
	if results[0].Outcome != TimedOut || results[1].Outcome != Survived {
		t.Fatalf("got %v then %v; the loop mutant in a.go must be gone before b.go is tested", results[0].Outcome, results[1].Outcome)
	}
}

func TestRunnerStopsATestRunWhenCancelled(t *testing.T) {
	root := writeTree(t, map[string]string{"go.mod": goModule, "a.go": goSource, "a_test.go": goTests})
	ms, _ := GoMutants("a.go", []byte(goSource))
	var loop Mutant
	for _, m := range ms {
		if m.Desc == "*= to /=" {
			loop = m
		}
	}
	ctx, cancel := context.WithCancel(context.Background())
	timer := time.AfterFunc(4*time.Second, cancel)
	defer timer.Stop()
	called := false
	r := &Runner{Root: root, Jobs: 1, Timeout: time.Minute, Done: func(Result) { called = true }}
	started := time.Now()
	_, err := r.Run(ctx, []Mutant{loop}, map[string]Target{"a.go": {Component: ".", Kind: "go"}})
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("got %v", err)
	}
	if called {
		t.Fatal("a run cut short by cancelling has no outcome to report")
	}
	if time.Since(started) > 30*time.Second {
		t.Fatal("cancelling must stop the test run, not wait for its timeout")
	}
}

func TestTimeoutFor(t *testing.T) {
	if got := (&Runner{}).timeoutFor(10 * time.Second); got != time.Minute {
		t.Errorf("default: %v, want 3 times 10s plus 30s", got)
	}
	if got := (&Runner{Timeout: 7 * time.Second}).timeoutFor(10 * time.Second); got != 7*time.Second {
		t.Errorf("set: %v", got)
	}
}

func TestCopyTreeFromARootNamedLikeBuildOutput(t *testing.T) {
	parent := t.TempDir()
	root := filepath.Join(parent, "target")
	if err := os.MkdirAll(filepath.Join(root, "src"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "src", "a.rs"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	dest := filepath.Join(t.TempDir(), "copy")
	if err := copyTree(root, dest); err != nil {
		t.Fatal(err)
	}
	if b, err := os.ReadFile(filepath.Join(dest, "src", "a.rs")); err != nil || string(b) != "x" {
		t.Fatalf("a checkout in a folder called target must still be copied: %q %v", b, err)
	}
}

func TestCopyTreeIntoAFolderInsideTheSource(t *testing.T) {
	root := writeTree(t, map[string]string{"a.go": "a", "build/keep.txt": "k"})
	dest := filepath.Join(root, "build", "mutate", "tree-0")
	if err := copyTree(root, dest); err != nil {
		t.Fatal(err)
	}
	if b, err := os.ReadFile(filepath.Join(dest, "a.go")); err != nil || string(b) != "a" {
		t.Fatalf("%q %v", b, err)
	}
	if _, err := os.Stat(filepath.Join(dest, "build", "mutate", "tree-0")); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("the copy must not contain itself: %v", err)
	}
	if _, err := os.Stat(filepath.Join(dest, "build", "keep.txt")); err != nil {
		t.Fatalf("other files of that folder are copied: %v", err)
	}
}
