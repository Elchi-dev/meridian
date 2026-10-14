package cli

import (
	"bytes"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// checkout writes a small repository with one Rust crate and one Go module
// whose line counts are known exactly.
func checkout(t *testing.T, files map[string]string) string {
	t.Helper()
	root := t.TempDir()
	files["sextant.meri"] = "stats {\n    min-ratio = 1.5\n}\n"
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

func runIn(dir string, args ...string) result {
	var out, errOut bytes.Buffer
	code := Run(args, Env{Dir: dir, Stdin: strings.NewReader(""), Stdout: &out, Stderr: &errOut, Version: "test"})
	return result{code: code, stdout: out.String(), stderr: errOut.String()}
}

var smallRepo = map[string]string{
	"boot/Cargo.toml": "[package]\nname = \"boot\"\n",
	"boot/src/main.rs": "unsafe fn a() {}\nfn b() {}\n\n#[cfg(test)]\nmod tests {\n" +
		"    #[test]\n    fn t() {\n        assert!(true);\n    }\n}\n",
	"tools/x/go.mod":       "module x\n",
	"tools/x/x.go":         "package x\n\n// F does f.\nfunc F() {}\n",
	"tools/x/x_test.go":    "package x\n\nimport \"testing\"\n\nfunc TestF(t *testing.T) {\n\tF()\n}\n",
	"docs/architecture.md": "# not code\n",
}

func copyRepo(m map[string]string) map[string]string {
	out := make(map[string]string, len(m))
	for k, v := range m {
		out[k] = v
	}
	return out
}

func TestStatsTable(t *testing.T) {
	root := checkout(t, copyRepo(smallRepo))
	r := runIn(filepath.Join(root, "docs"), "stats")
	if r.code != ExitOK {
		t.Fatalf("exit %d, stderr %q", r.code, r.stderr)
	}
	want := "" +
		"component  code  tests  ratio  unsafe\n" +
		"boot          2      7   3.50       1\n" +
		"tools/x       2      5   2.50       -\n" +
		"total         4     12   3.00       1\n"
	if r.stdout != want {
		t.Fatalf("got\n%s\nwant\n%s", r.stdout, want)
	}
}

func TestStatsFiles(t *testing.T) {
	root := checkout(t, copyRepo(smallRepo))
	r := runIn(root, "stats", "--files")
	gap := strings.Repeat(" ", 14)
	for _, want := range []string{
		"  boot/src/main.rs      2      7" + gap + "1\n",
		"  tools/x/x.go          2      0" + gap + "-\n",
		"  tools/x/x_test.go     0      5" + gap + "-\n",
	} {
		if !strings.Contains(r.stdout, want) {
			t.Errorf("missing %q in\n%s", want, r.stdout)
		}
	}
}

func TestStatsCheckPasses(t *testing.T) {
	root := checkout(t, copyRepo(smallRepo))
	r := runIn(root, "stats", "--check")
	if r.code != ExitOK || r.stderr != "" {
		t.Fatalf("exit %d, stderr %q", r.code, r.stderr)
	}
}

func TestStatsCheckFails(t *testing.T) {
	repo := copyRepo(smallRepo)
	repo["tools/x/more.go"] = "package x\n\nfunc G() int {\n\treturn 1\n}\n"
	root := checkout(t, repo)
	r := runIn(root, "stats", "--check")
	if r.code != ExitFailure {
		t.Fatalf("exit %d, want %d", r.code, ExitFailure)
	}
	want := "tools/x has 0.83 test lines per line of code and needs 1.50: write at least 4 more lines of tests.\n" +
		"sextant stats: 1 of 2 components have fewer than 1.50 test lines per line of code\n"
	if r.stderr != want {
		t.Fatalf("stderr\n%q\nwant\n%q", r.stderr, want)
	}
	if !strings.Contains(r.stdout, "tools/x       6      5   0.83") {
		t.Fatalf("the table is still printed before failing:\n%s", r.stdout)
	}
}

func TestStatsMinOverridesTheRule(t *testing.T) {
	root := checkout(t, copyRepo(smallRepo))
	if r := runIn(root, "stats", "--check", "--min", "3"); r.code != ExitFailure || !strings.Contains(r.stderr, "tools/x has 2.50 test lines per line of code and needs 3.00: write at least 1 more line of tests.") {
		t.Fatalf("exit %d, stderr %q", r.code, r.stderr)
	}
	if r := runIn(root, "stats", "--check", "--min", "2.5"); r.code != ExitOK {
		t.Fatalf("a ratio exactly at the minimum passes; exit %d, stderr %q", r.code, r.stderr)
	}
}

func TestStatsWithoutCheckNeverFails(t *testing.T) {
	root := checkout(t, copyRepo(smallRepo))
	if r := runIn(root, "stats", "--min", "100"); r.code != ExitOK {
		t.Fatalf("exit %d", r.code)
	}
}

func TestStatsComponentWithoutCode(t *testing.T) {
	root := checkout(t, map[string]string{"lib/empty/Cargo.toml": "[package]\nname = \"empty\"\n"})
	r := runIn(root, "stats", "--check")
	want := "" +
		"component  code  tests  ratio  unsafe\n" +
		"lib/empty     0      0      -       0\n" +
		"total         0      0      -       0\n"
	if r.code != ExitOK || r.stdout != want {
		t.Fatalf("exit %d\n%s", r.code, r.stdout)
	}
}

func TestStatsUsageErrors(t *testing.T) {
	root := checkout(t, copyRepo(smallRepo))
	if r := runIn(root, "stats", "--nope"); r.code != ExitUsage || !strings.Contains(r.stderr, "flag provided but not defined: -nope") {
		t.Errorf("unknown flag: exit %d, stderr %q", r.code, r.stderr)
	}
	if r := runIn(root, "stats", "kernel"); r.code != ExitUsage || r.stderr != "sextant stats: stats takes no arguments, got \"kernel\"\n" {
		t.Errorf("argument: exit %d, stderr %q", r.code, r.stderr)
	}
	if r := runIn(root, "stats", "--min", "lots"); r.code != ExitUsage {
		t.Errorf("bad number: exit %d", r.code)
	}
}

func TestStatsOutsideACheckout(t *testing.T) {
	r := runIn(t.TempDir(), "stats")
	if r.code != ExitFailure || !strings.Contains(r.stderr, "not inside a Meridian checkout") {
		t.Fatalf("exit %d, stderr %q", r.code, r.stderr)
	}
}

func TestStatsReportsSourcesItCannotRead(t *testing.T) {
	root := checkout(t, map[string]string{"k/Cargo.toml": "[package]\n", "k/src/lib.rs": "/* open"})
	r := runIn(root, "stats")
	if r.code != ExitFailure || r.stderr != "sextant stats: k/src/lib.rs:1:1: block comment is never closed\n" {
		t.Fatalf("exit %d, stderr %q", r.code, r.stderr)
	}
}

func TestWriteTableAlignsColumns(t *testing.T) {
	var b bytes.Buffer
	writeTable(&b, [][]string{{"a", "bb", ""}, {"ccc", "d", "e"}})
	if b.String() != "a    bb\nccc   d  e\n" {
		t.Fatalf("got %q", b.String())
	}
}

func TestPlural(t *testing.T) {
	for n, want := range map[int]string{0: "lines", 1: "line", 2: "lines"} {
		if got := plural(n, "line", "lines"); got != want {
			t.Errorf("plural(%d) = %q, want %q", n, got, want)
		}
	}
}
