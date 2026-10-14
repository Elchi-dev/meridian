package cli

import (
	"strings"
	"testing"
)

func TestLintCleanRepository(t *testing.T) {
	repo := copyRepo(smallRepo)
	repo["boot/src/main.rs"] = "mod raw;\nfn b() {}\n"
	repo["boot/src/raw.rs"] = "// SAFETY: nothing to uphold yet.\npub unsafe fn a() {}\n"
	root := checkout(t, repo)
	r := runIn(root, "lint")
	if r.code != ExitOK || r.stdout != "" || r.stderr != "" {
		t.Fatalf("exit %d, stdout %q, stderr %q", r.code, r.stdout, r.stderr)
	}
}

func TestLintReportsEveryProblem(t *testing.T) {
	repo := copyRepo(smallRepo)
	repo["README.md"] = "# x\n\nfast \u2014 small\n"
	repo["boot/src/extra.rs"] = "fn later() {\n    todo!()\n}\n"
	root := checkout(t, repo)
	r := runIn(root, "lint")
	if r.code != ExitFailure {
		t.Fatalf("exit %d, want %d", r.code, ExitFailure)
	}
	want := "README.md:3:6: dash: em dash; use a comma, a colon or a full stop, or a hyphen in a range\n" +
		"boot/src/extra.rs:2:5: stub: todo!() is a placeholder; write the code or leave the feature out\n" +
		"boot/src/main.rs:1:1: unsafe: unsafe code belongs in a raw module (raw.rs or raw/); move it there behind a safe function\n"
	if r.stdout != want {
		t.Fatalf("stdout\n%s\nwant\n%s", r.stdout, want)
	}
	if r.stderr != "sextant lint: found 3 problems\n" {
		t.Fatalf("stderr %q", r.stderr)
	}
}

func TestLintCountsOneProblem(t *testing.T) {
	repo := copyRepo(smallRepo)
	repo["boot/src/main.rs"] = "fn a() {}\n"
	repo["NOTES.md"] = "a \u2013 b\n"
	r := runIn(checkout(t, repo), "lint")
	if r.code != ExitFailure || r.stderr != "sextant lint: found 1 problem\n" {
		t.Fatalf("exit %d, stderr %q", r.code, r.stderr)
	}
}

func TestLintUsageErrors(t *testing.T) {
	root := checkout(t, copyRepo(smallRepo))
	if r := runIn(root, "lint", "--fix"); r.code != ExitUsage || !strings.Contains(r.stderr, "flag provided but not defined: -fix") {
		t.Errorf("unknown flag: exit %d, stderr %q", r.code, r.stderr)
	}
	if r := runIn(root, "lint", "kernel"); r.code != ExitUsage || r.stderr != "sextant lint: lint takes no arguments, got \"kernel\"\n" {
		t.Errorf("argument: exit %d, stderr %q", r.code, r.stderr)
	}
}

func TestLintOutsideACheckout(t *testing.T) {
	r := runIn(t.TempDir(), "lint")
	if r.code != ExitFailure || !strings.Contains(r.stderr, "not inside a Meridian checkout") {
		t.Fatalf("exit %d, stderr %q", r.code, r.stderr)
	}
}

func TestLintStopsOnSourceItCannotRead(t *testing.T) {
	root := checkout(t, map[string]string{"k/src/lib.rs": "\"open"})
	r := runIn(root, "lint")
	if r.code != ExitFailure || r.stderr != "sextant lint: k/src/lib.rs:1:1: string literal is never closed\n" {
		t.Fatalf("exit %d, stderr %q", r.code, r.stderr)
	}
}
