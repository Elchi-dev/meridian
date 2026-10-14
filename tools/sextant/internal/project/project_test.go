package project

import (
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func mkdir(t *testing.T, parts ...string) string {
	t.Helper()
	p := filepath.Join(parts...)
	if err := os.MkdirAll(p, 0o755); err != nil {
		t.Fatal(err)
	}
	return p
}

func touch(t *testing.T, parts ...string) {
	t.Helper()
	if err := os.WriteFile(filepath.Join(parts...), nil, 0o644); err != nil {
		t.Fatal(err)
	}
}

func TestFindRootFromTheRootItself(t *testing.T) {
	root := t.TempDir()
	touch(t, root, Marker)
	got, err := FindRoot(root)
	if err != nil || got != root {
		t.Fatalf("FindRoot = %q, %v; want %q", got, err, root)
	}
}

func TestFindRootFromADeepFolder(t *testing.T) {
	root := t.TempDir()
	touch(t, root, Marker)
	deep := mkdir(t, root, "kernel", "src", "arch")
	got, err := FindRoot(deep)
	if err != nil || got != root {
		t.Fatalf("FindRoot = %q, %v; want %q", got, err, root)
	}
}

func TestFindRootTakesTheNearestMarker(t *testing.T) {
	outer := t.TempDir()
	touch(t, outer, Marker)
	inner := mkdir(t, outer, "worktrees", "feature")
	touch(t, inner, Marker)
	start := mkdir(t, inner, "boot")
	got, err := FindRoot(start)
	if err != nil || got != inner {
		t.Fatalf("FindRoot = %q, %v; want the inner checkout %q", got, err, inner)
	}
}

func TestFindRootIgnoresAFolderNamedLikeTheMarker(t *testing.T) {
	outer := t.TempDir()
	touch(t, outer, Marker)
	inner := mkdir(t, outer, "a")
	mkdir(t, inner, Marker)
	got, err := FindRoot(inner)
	if err != nil || got != outer {
		t.Fatalf("FindRoot = %q, %v; want %q", got, err, outer)
	}
}

func TestFindRootResolvesRelativePaths(t *testing.T) {
	root := t.TempDir()
	touch(t, root, Marker)
	mkdir(t, root, "docs")
	t.Chdir(filepath.Join(root, "docs"))
	got, err := FindRoot(".")
	if err != nil {
		t.Fatal(err)
	}
	want, _ := filepath.EvalSymlinks(root)
	if gotReal, _ := filepath.EvalSymlinks(got); gotReal != want {
		t.Fatalf("FindRoot(\".\") = %q, want %q", got, root)
	}
}

func TestFindRootOutsideACheckout(t *testing.T) {
	start := mkdir(t, t.TempDir(), "elsewhere")
	_, err := FindRoot(start)
	if !errors.Is(err, ErrNotFound) {
		t.Fatalf("want ErrNotFound, got %v", err)
	}
	for _, part := range []string{"no sextant.meri in", start, "run sextant inside the repository"} {
		if !strings.Contains(err.Error(), part) {
			t.Errorf("message %q should mention %q", err.Error(), part)
		}
	}
}
