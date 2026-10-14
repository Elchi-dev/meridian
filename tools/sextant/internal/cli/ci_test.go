package cli

import (
	"bytes"
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func goBinary(t *testing.T) string {
	t.Helper()
	path, err := exec.LookPath("go")
	if err != nil {
		t.Fatal("the tests run under go, so go is on the PATH")
	}
	return path
}

func ciRunIn(root string, out *bytes.Buffer) ciRun {
	return ciRun{root: root, out: out, errOut: out}
}

func TestCIRunsStepsInOrderAndStopsAtTheFirstFailure(t *testing.T) {
	var out bytes.Buffer
	steps := []ciStep{
		{name: "first", dir: ".", args: []string{"go", "env", "GOOS"}},
		{name: "second", dir: ".", args: []string{"go", "tool", "no-such-tool"}},
		{name: "third", dir: ".", args: []string{"go", "version"}},
	}
	err := ciRunIn(t.TempDir(), &out).all(context.Background(), steps)
	if err == nil || !strings.HasPrefix(err.Error(), "second failed: ") {
		t.Fatalf("got %v", err)
	}
	got := out.String()
	for _, want := range []string{"== 1/3 first\n", "ok first in ", "== 2/3 second\n"} {
		if !strings.Contains(got, want) {
			t.Errorf("%q missing from:\n%s", want, got)
		}
	}
	if strings.Contains(got, "third") || strings.Contains(got, "ok second") || strings.Contains(got, "checks passed") {
		t.Errorf("nothing runs after a failure:\n%s", got)
	}
}

func TestCIReportsWhenEveryStepPassed(t *testing.T) {
	var out bytes.Buffer
	r := ciRunIn(t.TempDir(), &out)
	r.env = []string{"GOFLAGS=-mod=mod"}
	steps := []ciStep{{name: "flags", dir: ".", args: []string{"go", "env", "GOFLAGS"}}}
	if err := r.all(context.Background(), steps); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out.String(), "-mod=mod\n") || !strings.Contains(out.String(), "all 1 checks passed in ") {
		t.Fatalf("got:\n%s", out.String())
	}
}

func TestAStepRunsInItsFolderAndSextantIsThisSextant(t *testing.T) {
	root := t.TempDir()
	if err := os.MkdirAll(filepath.Join(root, "a", "b"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "a", "b", "go.mod"), []byte("module example.com/here\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	var out bytes.Buffer
	r := ciRunIn(root, &out)
	// sextant itself is the test binary here, so go stands in for it.
	r.self = goBinary(t)
	steps := []ciStep{
		{name: "where", dir: "a/b", args: []string{"go", "list", "-m"}},
		{name: "self", dir: ".", args: []string{"sextant", "version"}},
	}
	if err := r.all(context.Background(), steps); err != nil {
		t.Fatalf("%v\n%s", err, out.String())
	}
	if !strings.Contains(out.String(), "example.com/here\n") || !strings.Contains(out.String(), "go version go") {
		t.Fatalf("got:\n%s", out.String())
	}
}

func TestGoFilesAreCheckedAsGofmtWouldWriteThem(t *testing.T) {
	root := t.TempDir()
	write := func(name, src string) {
		path := filepath.Join(root, filepath.FromSlash(name))
		if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte(src), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	write("tidy.go", "package p\n\nimport (\n\t\"fmt\"\n\t\"os\"\n)\n\nfunc f() { fmt.Println(os.Args) }\n")
	write("notes.txt", "package   p")
	var out bytes.Buffer
	step := ciStep{name: "Go formatting", dir: ".", check: goFormatted}
	if err := ciRunIn(root, &out).all(context.Background(), []ciStep{step}); err != nil {
		t.Fatalf("tidy files: %v\n%s", err, out.String())
	}
	write("deep/messy.go", "package p\nfunc   g( ) {  }\n")
	write("unsorted.go", "package p\n\nimport (\n\t\"os\"\n\t\"fmt\"\n)\n\nvar _, _ = fmt.Println, os.Args\n")
	out.Reset()
	err := ciRunIn(root, &out).all(context.Background(), []ciStep{step})
	if err == nil || !strings.Contains(err.Error(), "gofmt would change 2 files") ||
		!strings.Contains(out.String(), "deep/messy.go\n") || !strings.Contains(out.String(), "unsorted.go\n") ||
		strings.Contains(out.String(), "tidy.go") {
		t.Fatalf("got %v\n%s", err, out.String())
	}
	write("broken.go", "package p\nfunc {")
	if err := goFormatted(root, &out); err == nil || !strings.Contains(err.Error(), "broken.go") {
		t.Errorf("a file Go cannot read: %v", err)
	}
	if err := goFormatted(filepath.Join(root, "missing"), &out); err == nil {
		t.Error("a folder that is not there")
	}
}

func TestOnGitHubEachStepIsAGroupOfTheLog(t *testing.T) {
	var out bytes.Buffer
	r := ciRunIn(t.TempDir(), &out)
	r.github = true
	steps := []ciStep{
		{name: "first", dir: ".", args: []string{"go", "env", "GOOS"}},
		{name: "second", dir: ".", args: []string{"go", "tool", "no-such-tool"}},
	}
	_ = r.all(context.Background(), steps)
	got := out.String()
	if !strings.HasPrefix(got, "::group::first\n") || strings.Count(got, "::endgroup::\n") != 2 ||
		!strings.Contains(got, "::group::second\n") || strings.Contains(got, "== ") {
		t.Fatalf("got:\n%s", got)
	}
}

// The workflow's linux job runs 'sextant ci' and no check of its own, so
// the list here is what CI does. The macOS and Windows job only tests
// sextant itself there.
func TestTheWorkflowRunsTheseSteps(t *testing.T) {
	workflow, err := os.ReadFile(filepath.Join("..", "..", "..", "..", ".github", "workflows", "ci.yml"))
	if err != nil {
		t.Fatal(err)
	}
	_, linux, _ := strings.Cut(string(workflow), "\n  linux:\n")
	linux, _, _ = strings.Cut(linux, "\n  mutate:\n")
	if !strings.Contains(linux, "go run ./cmd/sextant ci\n") {
		t.Errorf("the linux job should run sextant ci:\n%s", linux)
	}
	for _, rule := range []string{"cargo clippy", "cargo fmt", "cargo test", "go vet", "go test", "sextant lint", "sextant test"} {
		if strings.Contains(linux, rule) {
			t.Errorf("the linux job runs %q itself; it belongs in ciSteps", rule)
		}
	}
	steps := ciSteps()
	seen := map[string]bool{}
	for _, s := range steps {
		if seen[s.name] || (len(s.args) == 0) == (s.check == nil) {
			t.Errorf("step %q is there twice, or runs both or nothing", s.name)
		}
		seen[s.name] = true
		if s.check != nil {
			continue
		}
		switch s.args[0] {
		case "sextant", "cargo", "go":
		default:
			t.Errorf("step %q runs %s, which CI does not install", s.name, s.args[0])
		}
	}
	if steps[0].name != "project rules" || steps[len(steps)-1].name != "the QEMU test image" {
		t.Error("the quick rules come first and the QEMU test image last")
	}
}

// The whole command, in a repository of its own: it runs from any folder
// in it, gives Cargo the build folder the settings name, and groups the
// log on GitHub.
func TestCIRunsInTheRepositoryWithItsBuildFolder(t *testing.T) {
	needTool(t, "cargo")
	root := checkout(t, map[string]string{
		"sextant.local.meri": "build {\n    dir = \"out/cargo\"\n}\n",
		"Cargo.toml":         "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
		"src/lib.rs":         "",
	})
	steps := []ciStep{{name: "where Cargo builds", dir: ".", args: []string{"cargo", "metadata", "--no-deps", "--format-version", "1", "--offline"}}}
	ci := func() (string, error) {
		var out, errOut bytes.Buffer
		err := ciCommand(Env{Dir: filepath.Join(root, "src"), Stdout: &out, Stderr: &errOut}, nil, steps)
		return out.String(), err
	}
	t.Setenv("GITHUB_ACTIONS", "")
	got, err := ci()
	want := `"target_directory":"` + filepath.Join(root, "out", "cargo") + `"`
	if err != nil || !strings.Contains(got, want) || !strings.HasPrefix(got, "== 1/1 where Cargo builds\n") {
		t.Fatalf("got %v\n%s", err, got)
	}
	t.Setenv("GITHUB_ACTIONS", "true")
	if got, err := ci(); err != nil || !strings.HasPrefix(got, "::group::where Cargo builds\n") {
		t.Fatalf("on GitHub: %v\n%s", err, got)
	}
	var out bytes.Buffer
	err = ciCommand(Env{Dir: t.TempDir(), Stdout: &out, Stderr: &out}, nil, steps)
	if err == nil || out.Len() != 0 {
		t.Fatalf("outside a repository: %v\n%s", err, out.String())
	}
}

func TestCIListsItsSteps(t *testing.T) {
	r := run("ci", "--list")
	if r.code != ExitOK || strings.Count(r.stdout, "\n") != len(ciSteps()) {
		t.Fatalf("code %d:\n%s", r.code, r.stdout)
	}
	if !strings.Contains(r.stdout, "checked by sextant itself\n") || !strings.Contains(r.stdout, "cargo fmt --all -- --check\n") {
		t.Errorf("each line names the step and its command:\n%s", r.stdout)
	}
	if r := run("ci", "extra"); r.code != ExitUsage {
		t.Errorf("an argument: code %d", r.code)
	}
}
