package mutate

import (
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/Elchi-dev/meridian/tools/sextant/internal/srclex"
	"github.com/Elchi-dev/meridian/tools/sextant/internal/stats"
)

func sources() []stats.Source {
	return []stats.Source{
		{Path: "boot/src/main.rs", Lang: srclex.Rust, Component: "boot", Kind: "rust"},
		{Path: "boot/src/start.s", Lang: srclex.Asm, Component: "boot", Kind: "rust"},
		{Path: "boot/src/tests.rs", Lang: srclex.Rust, Component: "boot", Kind: "rust", AllTest: true},
		{Path: "lib/uefi/src/lib.rs", Lang: srclex.Rust, Component: "lib/uefi", Kind: "rust"},
		{Path: "tools/x/a.go", Lang: srclex.Go, Component: "tools/x", Kind: "go"},
		{Path: "tools/x/a_test.go", Lang: srclex.Go, Component: "tools/x", Kind: "go", AllTest: true},
		{Path: "tools/xy/b.go", Lang: srclex.Go, Component: "tools/xy", Kind: "go"},
	}
}

func paths(srcs []stats.Source) string {
	var out []string
	for _, s := range srcs {
		out = append(out, s.Path)
	}
	return strings.Join(out, " ")
}

func TestPickEverythingByDefault(t *testing.T) {
	got, err := Pick(sources(), nil, nil)
	if err != nil {
		t.Fatal(err)
	}
	if paths(got) != "boot/src/main.rs lib/uefi/src/lib.rs tools/x/a.go tools/xy/b.go" {
		t.Fatalf("got %s; assembly and test files are never mutated", paths(got))
	}
}

func TestPickByPath(t *testing.T) {
	tests := []struct {
		paths []string
		want  string
	}{
		{[]string{"."}, "boot/src/main.rs lib/uefi/src/lib.rs tools/x/a.go tools/xy/b.go"},
		{[]string{"tools/x"}, "tools/x/a.go"},
		{[]string{"tools/x/a.go"}, "tools/x/a.go"},
		{[]string{"boot", "tools/xy"}, "boot/src/main.rs tools/xy/b.go"},
		{[]string{"lib"}, "lib/uefi/src/lib.rs"},
	}
	for _, tt := range tests {
		got, err := Pick(sources(), tt.paths, nil)
		if err != nil || paths(got) != tt.want {
			t.Errorf("%v: %s, %v; want %s", tt.paths, paths(got), err, tt.want)
		}
	}
}

func TestPickRefusesPathsWithoutCode(t *testing.T) {
	for _, p := range []string{"docs", "tools/x/a_test.go", "boot/src/start.s", "tool"} {
		_, err := Pick(sources(), []string{"boot", p}, nil)
		if err == nil || err.Error() != p+" has no Rust or Go code to mutate" {
			t.Errorf("%s: got %v", p, err)
		}
	}
}

func TestPickOnlyChangedFiles(t *testing.T) {
	changed := map[string]bool{"tools/x/a.go": true, "README.md": true, "tools/x/a_test.go": true}
	got, err := Pick(sources(), nil, changed)
	if err != nil || paths(got) != "tools/x/a.go" {
		t.Fatalf("%s %v", paths(got), err)
	}
	got, err = Pick(sources(), nil, map[string]bool{})
	if err != nil || len(got) != 0 {
		t.Fatalf("nothing changed means nothing to mutate: %s %v", paths(got), err)
	}
	if _, err := Pick(sources(), []string{"boot"}, changed); err == nil {
		t.Fatal("a path whose files did not change has nothing to mutate")
	}
}

func TestGather(t *testing.T) {
	root := writeTree(t, map[string]string{
		"x/go.mod":       goModule,
		"x/a.go":         "package m\n\nvar b = 1 < 2\n",
		"k/Cargo.toml":   "[package]\nname = \"k\"\n",
		"k/src/lib.rs":   "const C: bool = true;\n#[cfg(test)]\nmod tests {\n    const D: bool = false;\n}\n",
		"x/a_test.go":    "package m\n",
		"k/src/notes.md": "1 + 1\n",
	})
	srcs, err := stats.Sources(os.DirFS(root))
	if err != nil {
		t.Fatal(err)
	}
	picked, err := Pick(srcs, nil, nil)
	if err != nil {
		t.Fatal(err)
	}
	ms, targets, err := Gather(root, picked)
	if err != nil {
		t.Fatal(err)
	}
	var got []string
	for _, m := range ms {
		got = append(got, m.String())
	}
	want := "k/src/lib.rs:1: true to false,x/a.go:3: < to <=,x/a.go:3: < to >="
	if strings.Join(got, ",") != want {
		t.Fatalf("got  %s\nwant %s", strings.Join(got, ","), want)
	}
	if targets["x/a.go"] != (Target{Component: "x", Kind: "go"}) || targets["k/src/lib.rs"] != (Target{Component: "k", Kind: "rust"}) || len(targets) != 2 {
		t.Fatalf("targets %v", targets)
	}
}

func TestGatherReportsBrokenAndMissingFiles(t *testing.T) {
	root := writeTree(t, map[string]string{"x/a.go": "package m\n\nfunc (\n", "k/src/lib.rs": "\"open"})
	_, _, err := Gather(root, []stats.Source{{Path: "x/a.go", Lang: srclex.Go}})
	if err == nil || !strings.HasPrefix(err.Error(), "x/a.go: ") {
		t.Fatalf("got %v", err)
	}
	_, _, err = Gather(root, []stats.Source{{Path: "k/src/lib.rs", Lang: srclex.Rust}})
	if err == nil || !strings.HasPrefix(err.Error(), "k/src/lib.rs: ") {
		t.Fatalf("got %v", err)
	}
	_, _, err = Gather(root, []stats.Source{{Path: "gone.go", Lang: srclex.Go}})
	if !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("got %v", err)
	}
	if _, err := os.Stat(filepath.Join(root, "x", "a.go")); err != nil {
		t.Fatal(err)
	}
}
