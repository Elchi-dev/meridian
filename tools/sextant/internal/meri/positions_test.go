package meri

import "testing"

// Columns count characters, so every error after multi-byte text must land
// on the character a person sees in an editor.
func TestColumnsCountCharactersNotBytes(t *testing.T) {
	tests := []struct {
		name      string
		src       string
		line, col int
		code      string
	}{
		{"two-byte letters in a string", "a = \"\u00e4\u00f6\u00fc\" @\n", 1, 11, CodeExpectedNewline},
		{"three-byte letters in a string", "a = \"\u4e16\u754c\" @\n", 1, 10, CodeExpectedNewline},
		{"four-byte emoji in a string", "a = \"\U0001F600\U0001F600\" @\n", 1, 10, CodeExpectedNewline},
		{"multi-byte text in a comment", "# \u00e4\U0001F600\u4e16 \x01\n", 1, 7, CodeControlCharacter},
		{"multi-byte label", "d \"\u00e9t\u00e9\" x\n", 1, 9, CodeExpectedBrace},
		{"multi-byte text block", "t = \"\"\"\n  \u00e4\u00e4\x01\n  \"\"\"\n", 2, 5, CodeControlCharacter},
		{"tab counts as one column", "a\t=\t@\n", 1, 5, CodeUnexpectedCharacter},
		{"byte order mark does not count", "\xef\xbb\xbf\u00e4", 1, 1, CodeUnexpectedCharacter},
		{"escapes count as the characters written", "a = \"\\u{1F600}\\n\" @\n", 1, 19, CodeExpectedNewline},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			e := parseError(t, tt.src)
			if e.Line != tt.line || e.Col != tt.col || e.Code != tt.code {
				t.Errorf("got %d:%d %s, want %d:%d %s", e.Line, e.Col, e.Code, tt.line, tt.col, tt.code)
			}
		})
	}
}

func TestLinesCountEveryLineEnding(t *testing.T) {
	tests := []struct {
		src  string
		line int
	}{
		{"a = 1\nb = 2\n@\n", 3},
		{"a = 1\r\nb = 2\r\n@\r\n", 3},
		{"a = 1\r\nb = 2\n@\n", 3},
		{"l = [\n1,\r\n2,\n]\n@\n", 5},
		{"t = \"\"\"\r\n  x\r\n  \"\"\"\r\n@\n", 4},
		{"# c\n\n\n# d\r\n@", 5},
		{"b {\n  c {\r\n  }\n}\n@\n", 5},
	}
	for _, tt := range tests {
		e := parseError(t, tt.src)
		if e.Line != tt.line || e.Col != 1 || e.Code != CodeUnexpectedCharacter {
			t.Errorf("%q: got %d:%d %s, want %d:1", tt.src, e.Line, e.Col, e.Code, tt.line)
		}
	}
}

// Only the first error in the document is reported.
func TestTheFirstErrorWins(t *testing.T) {
	tests := []struct {
		name      string
		src       string
		line, col int
		code      string
	}{
		{"a duplicate key before a broken value", "a = 1\na = @\n", 2, 1, CodeDuplicateKey},
		{"a duplicate label before broken contents", "d \"x\" { }\nd \"x\" { @ }\n", 2, 1, CodeDuplicateLabel},
		{"a name conflict before broken contents", "d = 1\nd { @ }\n", 2, 1, CodeNameConflict},
		{"an error inside a block before the unclosed block", "outer {\n  a = @\n", 2, 7, CodeUnexpectedCharacter},
		{"a broken value before the next line", "a = 01\nb = @\n", 1, 5, CodeInvalidNumber},
		{"a broken list item before the missing bracket", "l = [1, @\n", 1, 9, CodeUnexpectedCharacter},
		{"a bad character inside a text before its indentation", "t = \"\"\"\n x\n  \x01\n  \"\"\"\n", 3, 3, CodeControlCharacter},
		{"the first badly indented text line", "t = \"\"\"\n a\n b\n  \"\"\"\n", 2, 1, CodeTextIndent},
		{"too deep before a duplicate label", deepThenDuplicate(), 65, 1, CodeTooDeep},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			e := parseError(t, tt.src)
			if e.Line != tt.line || e.Col != tt.col || e.Code != tt.code {
				t.Errorf("got %d:%d %s (%s), want %d:%d %s", e.Line, e.Col, e.Code, e.Msg, tt.line, tt.col, tt.code)
			}
		})
	}
}

func deepThenDuplicate() string {
	s := ""
	for range 64 {
		s += "b {\n"
	}
	return s + "c \"x\" { }\n"
}

func TestNumbersAtTheEndOfTheFile(t *testing.T) {
	valid := map[string]string{
		"a = 0x":   "a quantity 0 x\n",
		"a = 0b":   "a quantity 0 b\n",
		"a = 0o":   "a quantity 0 o\n",
		"a = 1e":   "a quantity 1 e\n",
		"a = 1e+":  "",
		"a = 5%":   "a quantity 5 %\n",
		"a = -0":   "a int 0\n",
		"a = 0x1f": "a int 31\n",
		"a = 1.5":  "a float 1.5\n",
		"a = 2e3":  "a float 2e3\n",
	}
	for src, want := range valid {
		doc, err := Parse("eof.meri", []byte(src))
		if want == "" {
			if err == nil {
				t.Errorf("%q parsed, but + cannot follow a number", src)
			}
			continue
		}
		if err != nil {
			t.Errorf("%q: %v", src, err)
			continue
		}
		if got := Dump(doc); got != want {
			t.Errorf("%q dumps to %q, want %q", src, got, want)
		}
	}
	invalid := map[string]string{
		"a = 5.":   CodeInvalidNumber,
		"a = 1_":   CodeInvalidNumber,
		"a = -":    CodeInvalidNumber,
		"a = 0x1g": CodeUnitNotAllowed,
		"a = 0x1_": CodeInvalidNumber,
		"a = 0b1%": CodeUnitNotAllowed,
	}
	for src, code := range invalid {
		if e := parseError(t, src); e.Code != code || e.Col != 5 {
			t.Errorf("%q: got %d:%d %s, want 1:5 %s", src, e.Line, e.Col, e.Code, code)
		}
	}
}

func TestValuesAfterTheirKeysOnOneLine(t *testing.T) {
	doc := mustParse(t, "key    =\t\"v\"\nother=1\n")
	k, _ := doc.Get("key")
	o, _ := doc.Get("other")
	if k.Pos.Col != 10 || o.Pos.Col != 7 || o.Pos.Line != 2 {
		t.Fatalf("value positions %v and %v", k.Pos, o.Pos)
	}
}
