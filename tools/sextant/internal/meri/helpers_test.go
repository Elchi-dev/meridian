package meri

import (
	"strings"
	"testing"
	"unsafe"
)

// The character classes are checked against all 256 bytes, with the
// reference sets spelled out the long way.
func TestCharacterClasses(t *testing.T) {
	const (
		lower = "abcdefghijklmnopqrstuvwxyz"
		upper = "ABCDEFGHIJKLMNOPQRSTUVWXYZ"
		dec   = "0123456789"
	)
	classes := []struct {
		name string
		fn   func(byte) bool
		set  string
	}{
		{"isLetter", isLetter, lower + upper},
		{"isDigit", isDigit, dec},
		{"isKeyStart", isKeyStart, lower + upper + "_"},
		{"isKeyChar", isKeyChar, lower + upper + dec + "_-"},
		{"isHex", isHex, dec + "abcdefABCDEF"},
		{"isOctal", isOctal, "01234567"},
		{"isBinary", isBinary, "01"},
		{"isNumberTail", isNumberTail, lower + upper + dec + "_.%"},
	}
	for _, c := range classes {
		for b := range 256 {
			want := strings.IndexByte(c.set, byte(b)) >= 0
			if got := c.fn(byte(b)); got != want {
				t.Errorf("%s(%q) = %v, want %v", c.name, byte(b), got, want)
			}
		}
	}
}

func TestHexDigitValues(t *testing.T) {
	for b := range 256 {
		c := byte(b)
		got, ok := hexDigit(c)
		want := strings.IndexByte("0123456789abcdef", c)
		if want < 0 {
			want = strings.IndexByte("0123456789ABCDEF", c)
		}
		if ok != (want >= 0) || ok && got != rune(want) {
			t.Errorf("hexDigit(%q) = %d, %v; want %d", c, got, ok, want)
		}
	}
}

func TestSimpleEscapes(t *testing.T) {
	want := map[byte]rune{'\\': '\\', '"': '"', 'n': '\n', 'r': '\r', 't': '\t', '0': 0}
	for b := range 256 {
		r, ok := simpleEscape(byte(b))
		w, known := want[byte(b)]
		if ok != known || r != w {
			t.Errorf("simpleEscape(%q) = %q, %v; want %q, %v", byte(b), r, ok, w, known)
		}
	}
}

func TestDigitsEnd(t *testing.T) {
	tests := []struct {
		src   string
		start int
		want  int
	}{
		{"", 0, 0},
		{"123", 0, 3},
		{"123abc", 0, 3},
		{"1_000", 0, 5},
		{"1__0", 0, 1},
		{"1_", 0, 1},
		{"1_a", 0, 1},
		{"x12", 1, 3},
		{"12_34_56 ", 0, 8},
	}
	for _, tt := range tests {
		if got := digitsEnd([]byte(tt.src), tt.start, isDigit); got != tt.want {
			t.Errorf("digitsEnd(%q, %d) = %d, want %d", tt.src, tt.start, got, tt.want)
		}
	}
	if got := digitsEnd([]byte("ff_ee_g"), 0, isHex); got != 5 {
		t.Errorf("digitsEnd with hex digits = %d, want 5", got)
	}
}

func TestFloatText(t *testing.T) {
	tests := []struct{ in, want string }{
		{"1.5", "1.5"},
		{"1_000.000_1", "1000.0001"},
		{"6.022E23", "6.022e23"},
		{"1e+9", "1e9"},
		{"1E+9", "1e9"},
		{"-1.5e-3", "-1.5e-3"},
		{"2e1_0", "2e10"},
	}
	for _, tt := range tests {
		if got := floatText([]byte(tt.in)); got != tt.want {
			t.Errorf("floatText(%q) = %q, want %q", tt.in, got, tt.want)
		}
	}
}

func TestWithoutUnderscores(t *testing.T) {
	for in, want := range map[string]string{"": "", "123": "123", "1_000": "1000", "1_2_3": "123"} {
		if got := withoutUnderscores([]byte(in)); got != want {
			t.Errorf("withoutUnderscores(%q) = %q, want %q", in, got, want)
		}
	}
}

func TestOnlySpaces(t *testing.T) {
	for in, want := range map[string]bool{"": true, " ": true, "\t \t": true, " x": false, "  \u00a0": false, "\n": false} {
		if got := onlySpaces([]byte(in)); got != want {
			t.Errorf("onlySpaces(%q) = %v, want %v", in, got, want)
		}
	}
}

// Names repeat thousands of times in large documents; each distinct name is
// meant to be stored once.
func TestRepeatedNamesShareTheirStorage(t *testing.T) {
	doc := mustParse(t, "svc { mode = fast }\nsvc { mode = fast }\nmode = fast\n")
	svcs := doc.Blocks("svc")
	first, _ := svcs[0].Get("mode")
	second, _ := svcs[1].Get("mode")
	third, _ := doc.Get("mode")
	if unsafe.StringData(first.Str) != unsafe.StringData(second.Str) || unsafe.StringData(first.Str) != unsafe.StringData(third.Str) {
		t.Error("the word fast is stored more than once")
	}
	if unsafe.StringData(svcs[0].Items[0].Name) != unsafe.StringData(doc.Items[2].Name) {
		t.Error("the key mode is stored more than once")
	}
	if unsafe.StringData(svcs[0].Name) != unsafe.StringData(svcs[1].Name) {
		t.Error("the block name svc is stored more than once")
	}
}

func TestErrorCodesAreDistinctAndWellFormed(t *testing.T) {
	codes := []string{
		CodeInvalidUTF8, CodeControlCharacter, CodeLoneCarriageReturn, CodeUnexpectedCharacter,
		CodeExpectedEquals, CodeExpectedBrace, CodeExpectedValue, CodeExpectedNewline,
		CodeExpectedComma, CodeDuplicateKey, CodeNameConflict, CodeDuplicateLabel,
		CodeUnclosedBlock, CodeUnexpectedClose, CodeUnterminatedString, CodeInvalidEscape,
		CodeInvalidUnicodeEscape, CodeTextOpening, CodeTextIndent, CodeUnterminatedText,
		CodeUnterminatedList, CodeInvalidNumber, CodeNumberOverflow, CodeUnitNotAllowed, CodeTooDeep,
		CodeWrongType, CodeOutOfRange,
	}
	seen := map[string]bool{}
	for _, c := range codes {
		if seen[c] {
			t.Errorf("code %s is used twice", c)
		}
		seen[c] = true
		if c == "" || strings.Trim(c, "abcdefghijklmnopqrstuvwxyz0123456789-") != "" || strings.HasPrefix(c, "-") || strings.HasSuffix(c, "-") {
			t.Errorf("code %q must be lower-case words joined by single dashes", c)
		}
	}
}

func TestErrorStringFormat(t *testing.T) {
	e := &Error{File: "x/y.meri", Line: 7, Col: 12, Code: CodeTooDeep, Msg: "too deep"}
	if e.Error() != "x/y.meri:7:12: too deep" {
		t.Fatalf("Error() = %q", e.Error())
	}
	at := errorAt(Pos{File: "f", Line: 2, Col: 3}, CodeWrongType, "%s and %d", "a", 4)
	if *at != (Error{File: "f", Line: 2, Col: 3, Code: CodeWrongType, Msg: "a and 4"}) {
		t.Fatalf("errorAt built %+v", *at)
	}
}
