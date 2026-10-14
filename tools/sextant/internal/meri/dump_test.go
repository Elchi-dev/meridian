package meri

import "testing"

func TestQuoteUsesTheCanonicalEscapes(t *testing.T) {
	tests := []struct{ in, want string }{
		{"", `""`},
		{"plain", `"plain"`},
		{`back\slash`, `"back\\slash"`},
		{`say "hi"`, `"say \"hi\""`},
		{"a\nb\rc\td", `"a\nb\rc\td"`},
		{"nul\x00", `"nul\0"`},
		{"\x01", `"\u{1}"`},
		{"\x1f", `"\u{1f}"`},
		{"\x7f", `"\u{7f}"`},
		{" ~", `" ~"`},
		{"Gr\u00fcezi \u4e16 \U0001F600", "\"Gr\u00fcezi \u4e16 \U0001F600\""},
	}
	for _, tt := range tests {
		if got := quote(tt.in); got != tt.want {
			t.Errorf("quote(%q) = %s, want %s", tt.in, got, tt.want)
		}
	}
}

// The dump is checked here on documents built by hand, so a bug that the
// parser and the dump share cannot hide in the conformance suite.
func TestDumpOfAHandBuiltDocument(t *testing.T) {
	inner := &Block{Name: "rule"}
	inner.add(Item{Name: "n", Value: Value{Kind: KindInt, Int: -7}})
	second := &Block{Name: "rule", Label: "a\"b", Labelled: true}
	other := &Block{Name: "other"}
	other.add(Item{Name: "rule", Block: &Block{Name: "rule"}})

	root := &Block{}
	root.add(Item{Name: "rule", Block: inner})
	root.add(Item{Name: "rule", Block: second})
	root.add(Item{Name: "other", Block: other})
	root.add(Item{Name: "q", Value: Value{Kind: KindQuantity, Num: "1.5", Unit: "h"}})
	root.add(Item{Name: "f", Value: Value{Kind: KindFloat, Num: "2e3"}})
	root.add(Item{Name: "w", Value: Value{Kind: KindWord, Str: "kvm"}})
	root.add(Item{Name: "b", Value: Value{Kind: KindBool}})
	root.add(Item{Name: "l", Value: Value{Kind: KindList, List: []Value{
		{Kind: KindList, List: []Value{{Kind: KindString, Str: "x\ty"}}},
		{Kind: KindBool, Bool: true},
	}}})

	want := `rule#0 block
rule#0.n int -7
rule#1 block "a\"b"
other#0 block
other#0.rule#0 block
q quantity 1.5 h
f float 2e3
w word kvm
b bool false
l list 2
l[0] list 1
l[0][0] string "x\ty"
l[1] bool true
`
	if got := Dump(root); got != want {
		t.Errorf("dump differs\ngot:\n%s\nwant:\n%s", got, want)
	}
}

func TestDumpOfAnEmptyDocumentIsEmpty(t *testing.T) {
	if got := Dump(&Block{}); got != "" {
		t.Fatalf("Dump of an empty root = %q", got)
	}
}
