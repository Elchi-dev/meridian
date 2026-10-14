package meri

import (
	"errors"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
)

// The conformance suite lives at the repository root so the Rust reader can
// share it.
var suite = filepath.Join("..", "..", "..", "..", "tests", "meri")

func readVectors(t *testing.T, dir, want string) map[string][2][]byte {
	t.Helper()
	entries, err := os.ReadDir(filepath.Join(suite, dir))
	if err != nil {
		t.Fatalf("reading the conformance suite: %v", err)
	}
	out := map[string][2][]byte{}
	for _, e := range entries {
		name := e.Name()
		ext := filepath.Ext(name)
		if ext != ".meri" && ext != want {
			t.Errorf("%s/%s: unexpected file in the suite", dir, name)
			continue
		}
		data, err := os.ReadFile(filepath.Join(suite, dir, name))
		if err != nil {
			t.Fatal(err)
		}
		base := strings.TrimSuffix(name, ext)
		pair := out[base]
		if ext == ".meri" {
			pair[0] = data
		} else {
			pair[1] = data
		}
		out[base] = pair
	}
	for base, pair := range out {
		if pair[0] == nil || pair[1] == nil {
			t.Errorf("%s/%s: needs both %s.meri and %s%s", dir, base, base, base, want)
		}
	}
	return out
}

func TestConformanceValid(t *testing.T) {
	vectors := readVectors(t, "valid", ".dump")
	if len(vectors) < 15 {
		t.Fatalf("found only %d valid vectors; is the suite missing?", len(vectors))
	}
	for name, pair := range vectors {
		t.Run(name, func(t *testing.T) {
			doc, err := Parse(name+".meri", pair[0])
			if err != nil {
				t.Fatalf("Parse failed: %v", err)
			}
			if got := Dump(doc); got != string(pair[1]) {
				t.Errorf("dump differs\ngot:\n%s\nwant:\n%s", got, pair[1])
			}
		})
	}
}

func TestConformanceInvalid(t *testing.T) {
	vectors := readVectors(t, "invalid", ".error")
	if len(vectors) < 50 {
		t.Fatalf("found only %d invalid vectors; is the suite missing?", len(vectors))
	}
	for name, pair := range vectors {
		t.Run(name, func(t *testing.T) {
			line, col, code := parseExpectation(t, string(pair[1]))
			doc, err := Parse(name+".meri", pair[0])
			if err == nil {
				t.Fatalf("Parse accepted the document; dump:\n%s", Dump(doc))
			}
			var e *Error
			if !errors.As(err, &e) {
				t.Fatalf("error is %T, want *Error", err)
			}
			if e.Line != line || e.Col != col || e.Code != code {
				t.Errorf("got %d:%d %s (%s), want %d:%d %s", e.Line, e.Col, e.Code, e.Msg, line, col, code)
			}
			if e.File != name+".meri" {
				t.Errorf("error names file %q", e.File)
			}
			if e.Msg == "" {
				t.Error("error has no message")
			}
		})
	}
}

func parseExpectation(t *testing.T, s string) (int, int, string) {
	t.Helper()
	pos, code, ok := strings.Cut(strings.TrimSuffix(s, "\n"), " ")
	l, c, ok2 := strings.Cut(pos, ":")
	line, err1 := strconv.Atoi(l)
	col, err2 := strconv.Atoi(c)
	if !ok || !ok2 || err1 != nil || err2 != nil || code == "" {
		t.Fatalf("malformed expectation %q", s)
	}
	return line, col, code
}

// Every error code of the specification must be exercised by the suite, so
// a code cannot silently stop being produced.
func TestConformanceCoversEveryCode(t *testing.T) {
	codes := []string{
		CodeInvalidUTF8, CodeControlCharacter, CodeLoneCarriageReturn, CodeUnexpectedCharacter,
		CodeExpectedEquals, CodeExpectedBrace, CodeExpectedValue, CodeExpectedNewline,
		CodeExpectedComma, CodeDuplicateKey, CodeNameConflict, CodeDuplicateLabel,
		CodeUnclosedBlock, CodeUnexpectedClose, CodeUnterminatedString, CodeInvalidEscape,
		CodeInvalidUnicodeEscape, CodeTextOpening, CodeTextIndent, CodeUnterminatedText,
		CodeUnterminatedList, CodeInvalidNumber, CodeNumberOverflow, CodeUnitNotAllowed, CodeTooDeep,
	}
	used := map[string]bool{}
	for _, pair := range readVectors(t, "invalid", ".error") {
		_, _, code := parseExpectation(t, string(pair[1]))
		used[code] = true
	}
	for _, c := range codes {
		if !used[c] {
			t.Errorf("no invalid vector produces %s", c)
		}
		delete(used, c)
	}
	for c := range used {
		t.Errorf("the suite expects %s, which is not a code of the specification", c)
	}
}
