package mutate

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

const goodAllow = `# Mutants that cannot change behaviour.

allow {
    file = "a/b.go"
    line = "if x < y {"
    mutation = "< to <="
    reason = "x and y are never equal here"
}

allow {
    file = "k/src/lib.rs"
    line = "a + b"
    mutation = "+ to -"
    reason = "b is always zero"
}
`

func TestParseAllowances(t *testing.T) {
	got, err := ParseAllowances(AllowFile, []byte(goodAllow))
	if err != nil {
		t.Fatal(err)
	}
	if len(got) != 2 {
		t.Fatalf("%d entries", len(got))
	}
	a := got[0]
	if a.File != "a/b.go" || a.Line != "if x < y {" || a.Mutation != "< to <=" || a.Reason != "x and y are never equal here" {
		t.Fatalf("%+v", a)
	}
	if a.Pos.String() != "mutants.meri:3:1" || got[1].Pos.Line != 10 {
		t.Fatalf("positions: %v %v", a.Pos, got[1].Pos)
	}
}

func TestParseAllowancesRefusesMistakes(t *testing.T) {
	tests := []struct {
		src, want string
	}{
		{"x = 1\n", `mutants.meri:1:1: only allow blocks belong in mutants.meri, not "x"`},
		{"allow = 1\n", `mutants.meri:1:1: only allow blocks belong in mutants.meri, not "allow"`},
		{"permit {\n}\n", `mutants.meri:1:1: only allow blocks belong in mutants.meri, not "permit"`},
		{"allow \"x\" {\n}\n", "mutants.meri:1:1: allow takes no label"},
		{"allow {\n    file = \"a\"\n    extra = 1\n}\n", `mutants.meri:3:5: an allow has only file, line, mutation and reason, not "extra"`},
		{"allow {\n    file {\n    }\n}\n", `mutants.meri:2:5: an allow has only file, line, mutation and reason, not "file"`},
		{"allow {\n    file = \"a\"\n    line = \"b\"\n    mutation = \"c\"\n}\n", "mutants.meri:1:1: this allow has no reason; every allow needs file, line, mutation and reason"},
		{"allow {\n    line = \"b\"\n}\n", "mutants.meri:1:1: this allow has no file; every allow needs file, line, mutation and reason"},
		{"allow {\n    file = \"a\"\n    line = \"\"\n}\n", "mutants.meri:1:1: this allow has no line; every allow needs file, line, mutation and reason"},
		{"allow {\n    file = 4\n}\n", "mutants.meri:2:12: file must be text"},
		{"allow {\n", "mutants.meri"},
	}
	for _, tt := range tests {
		_, err := ParseAllowances(AllowFile, []byte(tt.src))
		if err == nil || !strings.HasPrefix(err.Error(), tt.want) {
			t.Errorf("%q: got %v, want %q", tt.src, err, tt.want)
		}
	}
}

func TestLoadAllowances(t *testing.T) {
	root := t.TempDir()
	got, err := LoadAllowances(root)
	if got != nil || err != nil {
		t.Fatalf("no file means nothing allowed: %v %v", got, err)
	}
	if err := os.WriteFile(filepath.Join(root, AllowFile), []byte(goodAllow), 0o644); err != nil {
		t.Fatal(err)
	}
	got, err = LoadAllowances(root)
	if len(got) != 2 || err != nil {
		t.Fatalf("%v %v", got, err)
	}
	odd := t.TempDir()
	if err := os.Mkdir(filepath.Join(odd, AllowFile), 0o755); err != nil {
		t.Fatal(err)
	}
	if _, err := LoadAllowances(odd); err == nil {
		t.Fatal("an allow list that cannot be read must be an error")
	}
	if err := os.WriteFile(filepath.Join(root, AllowFile), []byte("allow {\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := LoadAllowances(root); err == nil {
		t.Fatal("a broken allow list must be an error")
	}
}

func TestSplit(t *testing.T) {
	allow, err := ParseAllowances(AllowFile, []byte(goodAllow))
	if err != nil {
		t.Fatal(err)
	}
	ms := []Mutant{
		{Path: "a/b.go", LineText: "if x < y {", Desc: "< to <="},
		{Path: "a/b.go", LineText: "if x < y {", Desc: "< to >="},
		{Path: "a/b.go", LineText: "if x < z {", Desc: "< to <="},
		{Path: "a/c.go", LineText: "if x < y {", Desc: "< to <="},
	}
	run, allowed, stale := Split(allow, ms, map[string]bool{"a/b.go": true, "k/src/lib.rs": true})
	if len(allowed) != 1 || allowed[0] != ms[0] {
		t.Fatalf("allowed %v", allowed)
	}
	if len(run) != 3 || run[0] != ms[1] || run[1] != ms[2] || run[2] != ms[3] {
		t.Fatalf("run %v", run)
	}
	if len(stale) != 1 || stale[0].File != "k/src/lib.rs" {
		t.Fatalf("the lib.rs allow matches nothing and its file was checked: %v", stale)
	}
	_, _, stale = Split(allow, ms, map[string]bool{"a/b.go": true})
	if len(stale) != 0 {
		t.Fatalf("an allow for a file that was not checked is not stale: %v", stale)
	}
}

func TestOneAllowanceMayCoverTwoMutants(t *testing.T) {
	allow := []Allowance{{File: "a.go", Line: "x := a < b", Mutation: "< to <="}}
	ms := []Mutant{{Path: "a.go", LineText: "x := a < b", Desc: "< to <="}, {Path: "a.go", LineText: "x := a < b", Desc: "< to <="}}
	run, allowed, stale := Split(allow, ms, map[string]bool{"a.go": true})
	if len(run) != 0 || len(allowed) != 2 || len(stale) != 0 {
		t.Fatalf("%v %v %v", run, allowed, stale)
	}
}

func TestAllowEntryRoundTrips(t *testing.T) {
	ms := []Mutant{
		{Path: "tools/sextant/internal/x.go", LineText: `if s == "a\\b" {`, Desc: "== to !="},
		{Path: "k/src/lib.rs", LineText: "let t = a\t+ b;", Desc: "+ to -"},
		{Path: "k/src/odd.rs", LineText: "x\x01y\x7f", Desc: "! to nothing"},
	}
	var src strings.Builder
	for _, m := range ms {
		src.WriteString(m.AllowEntry())
	}
	allow, err := ParseAllowances(AllowFile, []byte(src.String()))
	if err != nil {
		t.Fatalf("%v\n%s", err, src.String())
	}
	for i, m := range ms {
		if !allow[i].matches(m) {
			t.Errorf("entry %d does not match its mutant: %+v", i, allow[i])
		}
		if allow[i].Reason != "why this change cannot alter behaviour" {
			t.Errorf("reason %q", allow[i].Reason)
		}
	}
	if !strings.Contains(ms[1].AllowEntry(), `line = "let t = a\t+ b;"`) {
		t.Fatalf("tabs are escaped:\n%s", ms[1].AllowEntry())
	}
	want := "allow {\n    file = \"k/src/lib.rs\"\n    line = \"let t = a\\t+ b;\"\n    mutation = \"+ to -\"\n    reason = \"why this change cannot alter behaviour\"\n}\n"
	if ms[1].AllowEntry() != want {
		t.Fatalf("got\n%s", ms[1].AllowEntry())
	}
}

func TestMeriString(t *testing.T) {
	tests := map[string]string{
		"plain":   `"plain"`,
		`a"b`:     `"a\"b"`,
		`a\b`:     `"a\\b"`,
		"a\tb":    `"a\tb"`,
		"\x00":    `"\u{0}"`,
		"\x1f":    `"\u{1f}"`,
		"\x7f":    `"\u{7f}"`,
		" ~":      `" ~"`,
		"\u00e9!": "\"\u00e9!\"",
	}
	for in, want := range tests {
		if got := meriString(in); got != want {
			t.Errorf("meriString(%q) = %s, want %s", in, got, want)
		}
	}
}
