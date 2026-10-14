package meri

import (
	"errors"
	"fmt"
	"testing"
)

func merged(t *testing.T, base, override string) *Block {
	t.Helper()
	b, err := Parse("base.meri", []byte(base))
	if err != nil {
		t.Fatalf("base: %v", err)
	}
	o, err := Parse("local.meri", []byte(override))
	if err != nil {
		t.Fatalf("override: %v", err)
	}
	return Merge(b, o)
}

func TestMergeRules(t *testing.T) {
	tests := []struct {
		name, base, override, want string
	}{
		{
			"entries replace in place and new ones go last",
			"a = 1\nb = 2\nc = 3\n", "b = 20\nd = 4\n",
			"a int 1\nb int 20\nc int 3\nd int 4\n",
		},
		{
			"an entry replaces every block of its name",
			"r { x = 1 }\nmid = 0\nr { x = 2 }\n", "r = off\n",
			"r word off\nmid int 0\n",
		},
		{
			"blocks replace an entry",
			"r = off\nz = 0\n", "r \"a\" { x = 1 }\nr { x = 2 }\n",
			"r#0 block \"a\"\nr#0.x int 1\nr#1 block\nr#1.x int 2\nz int 0\n",
		},
		{
			"labelled blocks merge by label, recursively",
			"d \"a\" { s = 1\n t = 1 }\nd \"b\" { s = 2 }\n", "d \"b\" { s = 20\n n { k = 1 } }\nd \"c\" { s = 3 }\n",
			"d#0 block \"a\"\nd#0.s int 1\nd#0.t int 1\nd#1 block \"b\"\nd#1.s int 20\nd#1.n#0 block\nd#1.n#0.k int 1\nd#2 block \"c\"\nd#2.s int 3\n",
		},
		{
			"one unlabelled block on each side merges",
			"q { cpus = 4\n mem = 1GiB }\n", "q { cpus = 8 }\n",
			"q#0 block\nq#0.cpus int 8\nq#0.mem quantity 1 GiB\n",
		},
		{
			"several unlabelled blocks are replaced in order and extra base blocks dropped",
			"r { n = 1 }\nmid = 0\nr { n = 2 }\n", "r { n = 3 }\n",
			"r#0 block\nr#0.n int 3\nmid int 0\n",
		},
		{
			"replacements keep the places of the base blocks",
			"r { n = 1 }\nmid = 0\nr { n = 2 }\n", "r { n = 3 }\nr { n = 4 }\n",
			"r#0 block\nr#0.n int 3\nmid int 0\nr#1 block\nr#1.n int 4\n",
		},
		{
			"extra override blocks follow the last base block",
			"r { n = 1 }\nmid = 0\nr { n = 2 }\nend = 0\n", "r { n = 3 }\nr { n = 4 }\nr { n = 5 }\n",
			"r#0 block\nr#0.n int 3\nmid int 0\nr#1 block\nr#1.n int 4\nr#2 block\nr#2.n int 5\nend int 0\n",
		},
		{
			"one base block and two override blocks are replaced",
			"r { n = 1 }\n", "r { n = 2 }\nr { n = 3 }\n",
			"r#0 block\nr#0.n int 2\nr#1 block\nr#1.n int 3\n",
		},
		{
			"one base block and two override blocks: replaced, not merged",
			"r { n = 1\n k = 9 }\nend = 0\n", "r { n = 2 }\nr { n = 3 }\n",
			"r#0 block\nr#0.n int 2\nr#1 block\nr#1.n int 3\nend int 0\n",
		},
		{
			"the empty label is still a label",
			"d \"\" { a = 1 }\n", "d { b = 2 }\n",
			"d#0 block \"\"\nd#0.a int 1\nd#1 block\nd#1.b int 2\n",
		},
		{
			"unlabelled override blocks next to labelled base blocks are added",
			"r \"a\" { }\n", "r { n = 1 }\n",
			"r#0 block \"a\"\nr#1 block\nr#1.n int 1\n",
		},
		{
			"labelled override blocks leave unlabelled base blocks alone",
			"r { n = 1 }\n", "r \"a\" { n = 2 }\n",
			"r#0 block\nr#0.n int 1\nr#1 block \"a\"\nr#1.n int 2\n",
		},
		{
			"base blocks the override does not touch stay",
			"keep { k = 1 }\n", "other = 2\n",
			"keep#0 block\nkeep#0.k int 1\nother int 2\n",
		},
		{
			"an empty override changes nothing",
			"a = 1\nb { c = 2 }\n", "",
			"a int 1\nb#0 block\nb#0.c int 2\n",
		},
		{
			"an empty base takes the override",
			"", "a = 1\nb { c = 2 }\n",
			"a int 1\nb#0 block\nb#0.c int 2\n",
		},
		{
			"mixed labelled and unlabelled in the base",
			"r \"a\" { n = 1 }\nr { n = 2 }\n", "r { n = 3 }\nr \"a\" { m = 4 }\n",
			"r#0 block \"a\"\nr#0.n int 1\nr#0.m int 4\nr#1 block\nr#1.n int 3\n",
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := Dump(merged(t, tt.base, tt.override)); got != tt.want {
				t.Errorf("got:\n%s\nwant:\n%s", got, tt.want)
			}
		})
	}
}

func TestMergeLeavesItsInputsAlone(t *testing.T) {
	base, _ := Parse("base.meri", []byte("a = 1\nq { cpus = 4 }\nr { }\nr { }\n"))
	over, _ := Parse("local.meri", []byte("q { cpus = 8 }\nr { x = 1 }\nz = 2\n"))
	beforeBase, beforeOver := Dump(base), Dump(over)
	out := Merge(base, over)
	if Dump(base) != beforeBase || Dump(over) != beforeOver {
		t.Fatal("Merge changed an input")
	}
	for _, b := range out.Blocks("q") {
		b.add(Item{Name: "extra", Value: Value{Kind: KindInt}})
	}
	for _, b := range out.Blocks("r") {
		b.add(Item{Name: "extra", Value: Value{Kind: KindInt}})
	}
	if Dump(base) != beforeBase || Dump(over) != beforeOver {
		t.Fatal("changing the merged document changed an input")
	}
}

func TestMergedValuesRememberTheirFile(t *testing.T) {
	out := merged(t, "q {\n  cpus = four\n  mem = lots\n}\n", "q {\n  cpus = many\n}\n")
	q := out.Blocks("q")[0]
	_, err := q.Int("cpus", 0)
	var e *Error
	if !errors.As(err, &e) || e.File != "local.meri" || e.Line != 2 {
		t.Errorf("cpus error = %v, want it to point at local.meri line 2", err)
	}
	_, err = q.Size("mem", 0)
	if !errors.As(err, &e) || e.File != "base.meri" || e.Line != 3 {
		t.Errorf("mem error = %v, want it to point at base.meri line 3", err)
	}
}

func TestMergeKeepsTheBaseRoot(t *testing.T) {
	base := &Block{Name: "outer", Label: "x", Labelled: true, Pos: Pos{File: "b", Line: 4, Col: 2}}
	out := Merge(base, &Block{Name: "other"})
	if out.Name != "outer" || out.Label != "x" || !out.Labelled || out.Pos != base.Pos {
		t.Fatalf("merged root = %+v", out)
	}
}

func TestMergeOfLargeBlocksUsesTheIndex(t *testing.T) {
	base := ""
	for i := range indexAfter {
		base += fmt.Sprintf("k%d = 1\n", i)
	}
	out := merged(t, base, "e = 2\nk3 = 3\n")
	if out.index == nil {
		t.Fatalf("a merged block with %d items has no index", len(out.Items))
	}
	if v, ok := out.Get("e"); !ok || v.Int != 2 {
		t.Errorf("e = %+v, %v after the merge", v, ok)
	}
	if v, _ := out.Get("k3"); v.Int != 3 {
		t.Errorf("k3 = %d after the merge", v.Int)
	}
}

// Found by FuzzParse: merging a document with itself moved the second A
// block in front of B, because replaced blocks all went to the first one's
// place. Each replacement now takes the place of its counterpart.
func TestMergingADocumentWithItselfChangesNothing(t *testing.T) {
	for _, src := range []string{
		"A{}\nB{}\nA{}",
		"r { n = 1 }\nx = 0\nr { n = 2 }\ny = 0\nr { n = 3 }\n",
		"d \"a\" { }\nd { }\nd \"b\" { }\nd { }\n",
	} {
		doc := mustParse(t, src)
		if got, want := Dump(Merge(doc, doc)), Dump(doc); got != want {
			t.Errorf("%q merged with itself:\n%s\nwant:\n%s", src, got, want)
		}
	}
}
