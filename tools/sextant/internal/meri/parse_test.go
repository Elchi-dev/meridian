package meri

import (
	"errors"
	"fmt"
	"strings"
	"testing"
)

func mustParse(t *testing.T, src string) *Block {
	t.Helper()
	doc, err := Parse("test.meri", []byte(src))
	if err != nil {
		t.Fatalf("Parse failed: %v", err)
	}
	return doc
}

func parseError(t *testing.T, src string) *Error {
	t.Helper()
	_, err := Parse("test.meri", []byte(src))
	if err == nil {
		t.Fatalf("Parse accepted %q", src)
	}
	var e *Error
	if !errors.As(err, &e) {
		t.Fatalf("error is %T, want *Error", err)
	}
	return e
}

func TestEmptyDocumentIsAnEmptyRoot(t *testing.T) {
	for _, src := range []string{"", "\r\n", "\xef\xbb\xbf", "# only a comment"} {
		doc := mustParse(t, src)
		if len(doc.Items) != 0 || doc.Name != "" || doc.Labelled {
			t.Errorf("%q: root = %+v", src, doc)
		}
		if doc.Pos != (Pos{File: "test.meri", Line: 1, Col: 1}) {
			t.Errorf("%q: root position %v", src, doc.Pos)
		}
	}
}

func TestPositionsOfItemsAndValues(t *testing.T) {
	doc := mustParse(t, "a = 1\n  blk \"x\" {\n      b = [1,\n        \"two\"]\n  }\nt = \"\"\"\n    z\n    \"\"\"\ns = [\"\u00fc\", 5]\n")
	pos := func(line, col int) Pos { return Pos{File: "test.meri", Line: line, Col: col} }
	checks := []struct {
		what      string
		got, want Pos
	}{
		{"item a", doc.Items[0].Pos, pos(1, 1)},
		{"value of a", doc.Items[0].Value.Pos, pos(1, 5)},
		{"item blk", doc.Items[1].Pos, pos(2, 3)},
		{"block blk", doc.Items[1].Block.Pos, pos(2, 3)},
		{"item b", doc.Items[1].Block.Items[0].Pos, pos(3, 7)},
		{"list b", doc.Items[1].Block.Items[0].Value.Pos, pos(3, 11)},
		{"first item of b", doc.Items[1].Block.Items[0].Value.List[0].Pos, pos(3, 12)},
		{"second item of b", doc.Items[1].Block.Items[0].Value.List[1].Pos, pos(4, 9)},
		{"text t", doc.Items[2].Value.Pos, pos(6, 5)},
		{"number after a multi-byte character", doc.Items[3].Value.List[1].Pos, pos(9, 11)},
	}
	for _, c := range checks {
		if c.got != c.want {
			t.Errorf("%s at %v, want %v", c.what, c.got, c.want)
		}
	}
	if b := doc.Items[1].Block; !b.Labelled || b.Label != "x" || b.Name != "blk" {
		t.Errorf("block = %+v", b)
	}
}

func TestErrorStringNamesFileLineAndColumn(t *testing.T) {
	e := parseError(t, "cpus = 4\nmemory = 1GiB\ncpus = 8\n")
	want := `test.meri:3:1: "cpus" is already set on line 1; remove one of them`
	if e.Error() != want {
		t.Fatalf("Error() = %q\nwant      %q", e.Error(), want)
	}
	if e.Code != CodeDuplicateKey {
		t.Fatalf("code = %s", e.Code)
	}
}

func TestErrorMessagesSayWhatHappened(t *testing.T) {
	tests := []struct {
		src  string
		want string
	}{
		{"q = 1\nq { }\n", `"q" is already set on line 1; a name is either a setting or a block`},
		{"q { }\nq = 1\n", `"q" is already the name of a block on line 1`},
		{"d \"x\" { }\nd \"x\" { }\n", `block d "x" is already defined on line 1; give this one another label`},
		{"d \"tab\\t\" { }\nd \"tab\\t\" { }\n", `block d "tab\t" is already defined`},
		{"outer {\n", `block "outer" is never closed; add a } for it`},
		{"}\n", "this } closes no block; remove it"},
		{"key\n", `expected = or { after "key", found the end of the line`},
		{"key", `expected = or { after "key", found the end of the file`},
		{"key # c\n", `expected = or { after "key", found a comment`},
		{"key: 1\n", `expected = or { after "key", found ':'`},
		{"d x y\n", `expected { after the label of block "d", found 'y'`},
		{"a = ", "expected a value, found the end of the file"},
		{"a =\n", "expected a value, found the end of the line"},
		{"a =\r\n", "expected a value, found the end of the line"},
		{"a = # c\n", "expected a value, found a comment"},
		{"a = [1,,]\n", "expected a value, found ','"},
		{"a = @\n", "'@' cannot start a value"},
		{"@\n", "unexpected '@'; a line holds a setting such as name = value"},
		{"a = 1 b\n", "unexpected 'b' after the end of the setting; start a new line"},
		{"a = 1 \u00e4\n", "unexpected '\u00e4' after the end of the setting"},
		{"a = \"x\n", "this string is never closed on its line"},
		{"a = \"x", "this string is never closed; add a \" at its end"},
		{"a = \"\\q\"\n", "a backslash followed by 'q' is not an escape"},
		{"a = \"\\", "a backslash followed by the end of the file is not an escape"},
		{"a = \"\\\n\"\n", "a backslash followed by the end of the line is not an escape"},
		{"a = \"\\\r\n\"\n", "a backslash followed by the end of the line is not an escape"},
		{"a = \"\\u{zz}\"\n", "\\u needs 1 to 6 hexadecimal digits"},
		{"t = \"\"\" x\n", "nothing may follow the opening \"\"\" on its line"},
		{"t = \"\"\"\n x\n", "this text is never closed"},
		{"t = \"\"\"\n  a\n b\n  \"\"\"\n", "indented less than the closing"},
		{"l = [1\n", "this list is never closed; add a ] at its end"},
		{"l = [1 2]\n", "expected , or ] after the list item, found '2'"},
		{"a = -x\n", "a minus sign must be followed directly by a digit"},
		{"a = 01\n", "a number cannot start with 0 unless it is 0"},
		{"a = 1.\n", "a decimal point needs digits on both sides"},
		{"a = 1__2\n", "this number has extra characters after it"},
		{"a = 0x1_\n", "this number has extra characters after it; check for the misplaced '_'"},
		{"a = 0x1.5\n", "this number has extra characters after it; check for the misplaced '.'"},
		{"a = 0x10G\n", "cannot carry a unit"},
		{"a = 9223372036854775808\n", "9223372036854775808 is outside the range of a 64-bit integer"},
		{"a = -0x8000_0000_0000_0001\n", "-0x8000_0000_0000_0001 is outside the range"},
		{"a = 1e999\n", "1e999 is too large for a 64-bit float"},
		{"a = 1\x01\n", "control character U+0001 is not allowed; write it as an escape inside a string, such as \\u{1}"},
		{"a = \"\xff\"\n", "the file is not valid UTF-8 here"},
		{"a = 1\rb", "a carriage return must be followed by a line feed"},
		{strings.Repeat("[", 0) + strings.Repeat("b {\n", 65), "nest more than 64 levels deep"},
	}
	for _, tt := range tests {
		e := parseError(t, tt.src)
		if !strings.Contains(e.Msg, tt.want) {
			t.Errorf("%q: message %q does not contain %q", tt.src, e.Msg, tt.want)
		}
		if strings.ContainsAny(e.Msg, "\u2013\u2014") {
			t.Errorf("%q: message contains a dash: %q", tt.src, e.Msg)
		}
	}
}

func TestGetAndBlocks(t *testing.T) {
	doc := mustParse(t, "name = x\nrule { n = 1 }\nrule \"b\" { n = 2 }\nrule { n = 3 }\n")
	if v, ok := doc.Get("name"); !ok || v.Kind != KindWord || v.Str != "x" {
		t.Errorf("Get(name) = %+v, %v", v, ok)
	}
	if _, ok := doc.Get("rule"); ok {
		t.Error("Get(rule) found a value for a block name")
	}
	if _, ok := doc.Get("missing"); ok {
		t.Error("Get(missing) found something")
	}
	rules := doc.Blocks("rule")
	if len(rules) != 3 {
		t.Fatalf("Blocks(rule) returned %d blocks", len(rules))
	}
	for i, want := range []int64{1, 2, 3} {
		if n, _ := rules[i].Int("n", 0); n != want {
			t.Errorf("rule %d has n = %d, want %d", i, n, want)
		}
	}
	if got := doc.Blocks("name"); got != nil {
		t.Errorf("Blocks(name) = %v for an entry", got)
	}
	if got := doc.Blocks("missing"); got != nil {
		t.Errorf("Blocks(missing) = %v", got)
	}
	if b, ok := doc.LabelledBlock("rule", "b"); !ok || b != rules[1] {
		t.Errorf("LabelledBlock(rule, b) = %v, %v", b, ok)
	}
	if _, ok := doc.LabelledBlock("rule", ""); ok {
		t.Error("an unlabelled block matched the empty label")
	}
	if _, ok := doc.LabelledBlock("rule", "c"); ok {
		t.Error("LabelledBlock found a label that does not exist")
	}
}

func TestEmptyLabelIsALabel(t *testing.T) {
	doc := mustParse(t, "d \"\" { }\nd { }\n")
	if b, ok := doc.LabelledBlock("d", ""); !ok || !b.Labelled {
		t.Fatalf("the block labelled \"\" was not found")
	}
	if e := parseError(t, "d \"\" { }\nd \"\" { }\n"); e.Code != CodeDuplicateLabel {
		t.Fatalf("repeating the empty label gave %s", e.Code)
	}
}

func TestIndexIsBuiltOnlyForLargeBlocks(t *testing.T) {
	var small strings.Builder
	for i := range indexAfter {
		fmt.Fprintf(&small, "k%d = %d\n", i, i)
	}
	if doc := mustParse(t, small.String()); doc.index != nil {
		t.Errorf("a block with %d items built an index", indexAfter)
	}
	// Repeated blocks come first, so a wrong index would point at the last one.
	large := strings.Repeat("rule { }\n", 5) + "d = 4\n"
	for i := 7; i <= indexAfter+1; i++ {
		large += fmt.Sprintf("n%d = %d\n", i, i)
	}
	doc := mustParse(t, large)
	if len(doc.Items) != indexAfter+1 {
		t.Fatalf("the large block has %d items, want %d", len(doc.Items), indexAfter+1)
	}
	if doc.index == nil {
		t.Fatalf("a block with %d items has no index", len(doc.Items))
	}
	if doc.index["rule"] != 0 {
		t.Errorf("index points rule at item %d, want the first one", doc.index["rule"])
	}
	if n := len(doc.Blocks("rule")); n != 5 {
		t.Errorf("Blocks(rule) found %d blocks through the index, want 5", n)
	}
	if v, ok := doc.Get("d"); !ok || v.Int != 4 {
		t.Errorf("Get(d) through the index = %+v, %v", v, ok)
	}
	if _, ok := doc.Get("zzz"); ok {
		t.Error("Get through the index found a missing key")
	}
}

func TestDuplicatesAreFoundInLargeBlocks(t *testing.T) {
	var src strings.Builder
	for i := range 100 {
		fmt.Fprintf(&src, "key%d = %d\n", i, i)
	}
	base := src.String()
	tests := []struct {
		add  string
		code string
		line int
	}{
		{"key7 = 1\n", CodeDuplicateKey, 101},
		{"key99 = 1\n", CodeDuplicateKey, 101},
		{"key3 { }\n", CodeNameConflict, 101},
		{"d \"x\" { }\nd \"y\" { }\nd \"x\" { }\n", CodeDuplicateLabel, 103},
		{"d { }\nd = 1\n", CodeNameConflict, 102},
	}
	for _, tt := range tests {
		e := parseError(t, base+tt.add)
		if e.Code != tt.code || e.Line != tt.line {
			t.Errorf("adding %q: got %s on line %d, want %s on line %d", tt.add, e.Code, e.Line, tt.code, tt.line)
		}
	}
}

func TestValueFields(t *testing.T) {
	doc := mustParse(t, "i = 0xff\nf = 1.5\nq = 512MiB\nh = 1.5h\nz = 1e-400\nw = kvm\nt = true\nn = false\ns = \"x\"\n")
	get := func(k string) Value {
		v, ok := doc.Get(k)
		if !ok {
			t.Fatalf("missing %s", k)
		}
		return v
	}
	if v := get("i"); v.Kind != KindInt || v.Int != 255 || v.Num != "255" || v.Unit != "" {
		t.Errorf("i = %+v", v)
	}
	if v := get("f"); v.Kind != KindFloat || v.Float != 1.5 || v.Num != "1.5" {
		t.Errorf("f = %+v", v)
	}
	if v := get("q"); v.Kind != KindQuantity || v.Int != 512 || v.Num != "512" || v.Unit != "MiB" {
		t.Errorf("q = %+v", v)
	}
	if v := get("h"); v.Kind != KindQuantity || v.Float != 1.5 || v.Num != "1.5" || v.Unit != "h" {
		t.Errorf("h = %+v", v)
	}
	if v := get("z"); v.Kind != KindFloat || v.Float != 0 {
		t.Errorf("z = %+v", v)
	}
	if v := get("w"); v.Kind != KindWord || v.Str != "kvm" {
		t.Errorf("w = %+v", v)
	}
	if v := get("t"); v.Kind != KindBool || !v.Bool {
		t.Errorf("t = %+v", v)
	}
	if v := get("n"); v.Kind != KindBool || v.Bool {
		t.Errorf("n = %+v", v)
	}
	if v := get("s"); v.Kind != KindString || v.Str != "x" {
		t.Errorf("s = %+v", v)
	}
}

func TestStringsWithEscapesKeepTheTextAround(t *testing.T) {
	doc := mustParse(t, "a = \"one\\ttwo\\u{e9}three\"\nb = \"\\n\"\nc = \"tail\\\\\"\n")
	for key, want := range map[string]string{"a": "one\ttwo\u00e9three", "b": "\n", "c": "tail\\"} {
		if v, _ := doc.Get(key); v.Str != want {
			t.Errorf("%s = %q, want %q", key, v.Str, want)
		}
	}
}

func TestKindNames(t *testing.T) {
	want := map[Kind]string{
		KindBool: "bool", KindInt: "int", KindFloat: "float", KindQuantity: "quantity",
		KindString: "string", KindWord: "word", KindList: "list", 0: "kind(0)", 99: "kind(99)",
		Kind(len(kindNames)): "kind(8)",
	}
	for k, name := range want {
		if k.String() != name {
			t.Errorf("Kind(%d).String() = %q, want %q", k, k.String(), name)
		}
	}
}

func TestPosString(t *testing.T) {
	if s := (Pos{File: "a.meri", Line: 12, Col: 3}).String(); s != "a.meri:12:3" {
		t.Fatalf("Pos.String() = %q", s)
	}
}

func TestDepthLimitCountsListsAndBlocks(t *testing.T) {
	ok := strings.Repeat("b {\n", 62) + "a = [[1]]\n" + strings.Repeat("}\n", 62)
	mustParse(t, ok)
	deep := strings.Repeat("b {\n", 63) + "a = [[1]]\n" + strings.Repeat("}\n", 63)
	if e := parseError(t, deep); e.Code != CodeTooDeep || e.Line != 64 || e.Col != 6 {
		t.Fatalf("got %d:%d %s", e.Line, e.Col, e.Code)
	}
	if MaxDepth != 64 {
		t.Fatalf("MaxDepth = %d; the specification fixes it at 64", MaxDepth)
	}
}

// In a block large enough to be indexed, an unlabelled block must not stand
// in for the empty label.
func TestUnlabelledBlocksAreNotTheEmptyLabelInLargeBlocks(t *testing.T) {
	src := "d { }\n"
	for i := range indexAfter {
		src += fmt.Sprintf("k%d = %d\n", i, i)
	}
	doc := mustParse(t, src+"d \"\" { x = 1 }\n")
	if doc.labels == nil {
		t.Fatal("the block was not indexed")
	}
	b, ok := doc.LabelledBlock("d", "")
	if !ok || !b.Labelled || len(b.Items) != 1 {
		t.Fatalf("LabelledBlock(d, \"\") = %+v, %v; want the labelled block", b, ok)
	}
	if e := parseError(t, src+"d \"\" { }\nd \"\" { }\n"); e.Code != CodeDuplicateLabel {
		t.Fatalf("a repeated empty label in a large block gave %s", e.Code)
	}
}
