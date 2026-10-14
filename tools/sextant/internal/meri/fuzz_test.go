package meri

import (
	"bytes"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"unicode/utf8"
)

var parseCodes = map[string]bool{
	CodeInvalidUTF8: true, CodeControlCharacter: true, CodeLoneCarriageReturn: true,
	CodeUnexpectedCharacter: true, CodeExpectedEquals: true, CodeExpectedBrace: true,
	CodeExpectedValue: true, CodeExpectedNewline: true, CodeExpectedComma: true,
	CodeDuplicateKey: true, CodeNameConflict: true, CodeDuplicateLabel: true,
	CodeUnclosedBlock: true, CodeUnexpectedClose: true, CodeUnterminatedString: true,
	CodeInvalidEscape: true, CodeInvalidUnicodeEscape: true, CodeTextOpening: true,
	CodeTextIndent: true, CodeUnterminatedText: true, CodeUnterminatedList: true,
	CodeInvalidNumber: true, CodeNumberOverflow: true, CodeUnitNotAllowed: true, CodeTooDeep: true,
}

// lineLengths returns the number of characters on each line, the way
// columns count them.
func lineLengths(src []byte) []int {
	src = bytes.TrimPrefix(src, []byte{0xEF, 0xBB, 0xBF})
	var out []int
	for _, line := range bytes.Split(src, []byte("\n")) {
		out = append(out, utf8.RuneCount(bytes.TrimSuffix(line, []byte("\r"))))
	}
	return out
}

func checkPos(t *testing.T, lengths []int, what string, p Pos) {
	t.Helper()
	if p.Line < 1 || p.Line > len(lengths) || p.Col < 1 || p.Col > lengths[p.Line-1]+1 {
		t.Fatalf("%s at %d:%d is outside the document (%d lines)", what, p.Line, p.Col, len(lengths))
	}
}

func before(a, b Pos) bool { return a.Line < b.Line || a.Line == b.Line && a.Col < b.Col }

func checkValue(t *testing.T, lengths []int, v Value) {
	t.Helper()
	checkPos(t, lengths, "a value", v.Pos)
	prev := v.Pos
	for _, e := range v.List {
		if !before(prev, e.Pos) {
			t.Fatalf("list item at %v does not come after %v", e.Pos, prev)
		}
		checkValue(t, lengths, e)
		prev = e.Pos
	}
}

func checkBlock(t *testing.T, lengths []int, b *Block, depth int) {
	t.Helper()
	if depth > MaxDepth {
		t.Fatalf("a block nests %d levels deep", depth)
	}
	prev := b.Pos
	for i, it := range b.Items {
		checkPos(t, lengths, "item "+it.Name, it.Pos)
		if (depth > 0 || i > 0) && !before(prev, it.Pos) {
			t.Fatalf("item %s at %v does not come after %v", it.Name, it.Pos, prev)
		}
		prev = it.Pos
		if it.Block != nil {
			checkBlock(t, lengths, it.Block, depth+1)
			continue
		}
		if !before(it.Pos, it.Value.Pos) {
			t.Fatalf("value of %s at %v does not come after its key", it.Name, it.Value.Pos)
		}
		checkValue(t, lengths, it.Value)
	}
}

func FuzzParse(f *testing.F) {
	for _, dir := range []string{"valid", "invalid"} {
		files, _ := filepath.Glob(filepath.Join(suite, dir, "*.meri"))
		for _, name := range files {
			data, err := os.ReadFile(name)
			if err != nil {
				f.Fatal(err)
			}
			f.Add(data)
		}
	}
	f.Fuzz(func(t *testing.T, src []byte) {
		lengths := lineLengths(src)
		doc, err := Parse("fuzz.meri", src)
		if err != nil {
			var e *Error
			if !errors.As(err, &e) {
				t.Fatalf("error %v is %T, not *Error", err, err)
			}
			if !parseCodes[e.Code] {
				t.Fatalf("unknown code %q", e.Code)
			}
			if e.Msg == "" || strings.ContainsAny(e.Msg, "\u2012\u2013\u2014\u2015") {
				t.Fatalf("bad message %q", e.Msg)
			}
			checkPos(t, lengths, "the error", Pos{Line: e.Line, Col: e.Col})
			return
		}
		checkBlock(t, lengths, doc, 0)
		dump := Dump(doc)
		if again := Dump(doc); again != dump {
			t.Fatal("two dumps of one document differ")
		}
		empty := &Block{}
		for name, merged := range map[string]*Block{
			"over an empty document": Merge(empty, doc),
			"with an empty override": Merge(doc, empty),
			"with itself":            Merge(doc, doc),
		} {
			if got := Dump(merged); got != dump {
				t.Fatalf("merging %s changed the document\nbefore:\n%s\nafter:\n%s", name, dump, got)
			}
		}
	})
}
