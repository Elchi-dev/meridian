package meri

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

type fence struct {
	lang, body, before string
}

// fences returns the fenced code blocks of a Markdown file with the prose
// that comes right before each one.
func fences(t *testing.T, path string) []fence {
	t.Helper()
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var out []fence
	parts := strings.Split(string(data), "```")
	for i := 1; i+1 < len(parts); i += 2 {
		lang, body, _ := strings.Cut(parts[i], "\n")
		out = append(out, fence{lang: strings.TrimSpace(lang), body: body, before: parts[i-1]})
	}
	return out
}

var specPath = filepath.Join("..", "..", "..", "..", "docs", "formats", "meri.md")

// Every example in the specification must be a valid document, so the text
// people read cannot drift away from what the parser accepts.
func TestEveryExampleInTheSpecificationParses(t *testing.T) {
	n := 0
	for _, f := range fences(t, specPath) {
		if f.lang != "meri" {
			continue
		}
		n++
		if _, err := Parse("spec example", []byte(f.body)); err != nil {
			t.Errorf("example %d does not parse: %v\n%s", n, err, f.body)
		}
	}
	if n < 4 {
		t.Fatalf("found only %d meri examples in the specification", n)
	}
}

func TestTheDumpExampleInTheSpecificationIsExact(t *testing.T) {
	fs := fences(t, specPath)
	for i := 0; i+1 < len(fs); i++ {
		if fs[i].lang != "meri" || !strings.Contains(fs[i+1].before, "dumps to") {
			continue
		}
		doc, err := Parse("spec example", []byte(fs[i].body))
		if err != nil {
			t.Fatal(err)
		}
		if got := Dump(doc); got != fs[i+1].body {
			t.Fatalf("the specification says the example dumps to\n%s\nbut it dumps to\n%s", fs[i+1].body, got)
		}
		return
	}
	t.Fatal("the specification has no example followed by \"dumps to\"")
}

// The table of error codes in the specification and the constants here must
// list the same codes.
func TestTheSpecificationListsEveryErrorCode(t *testing.T) {
	data, err := os.ReadFile(specPath)
	if err != nil {
		t.Fatal(err)
	}
	spec := string(data)
	codes := []string{
		CodeInvalidUTF8, CodeControlCharacter, CodeLoneCarriageReturn, CodeUnexpectedCharacter,
		CodeExpectedEquals, CodeExpectedBrace, CodeExpectedValue, CodeExpectedNewline,
		CodeExpectedComma, CodeDuplicateKey, CodeNameConflict, CodeDuplicateLabel,
		CodeUnclosedBlock, CodeUnexpectedClose, CodeUnterminatedString, CodeInvalidEscape,
		CodeInvalidUnicodeEscape, CodeTextOpening, CodeTextIndent, CodeUnterminatedText,
		CodeUnterminatedList, CodeInvalidNumber, CodeNumberOverflow, CodeUnitNotAllowed, CodeTooDeep,
	}
	for _, c := range codes {
		if !strings.Contains(spec, "| `"+c+"` |") {
			t.Errorf("the error table of the specification has no row for %s", c)
		}
	}
	for _, c := range []string{CodeWrongType, CodeOutOfRange} {
		if !strings.Contains(spec, "`"+c+"`") {
			t.Errorf("the specification never names the reader code %s", c)
		}
	}
	_, errorsSection, _ := strings.Cut(spec, "## 12. Errors")
	errorsSection, _, _ = strings.Cut(errorsSection, "## 13.")
	rows := strings.Count(errorsSection, "\n| `")
	if rows != len(codes) {
		t.Errorf("the specification has %d code rows, the package %d codes", rows, len(codes))
	}
}
