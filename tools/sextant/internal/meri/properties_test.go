package meri

import (
	"errors"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"testing"
)

// The specification asks every message to say what to do. These are the
// words each code's message must contain, checked against every invalid
// document of the conformance suite.
var adviceByCode = map[string]string{
	CodeInvalidUTF8:          "save it as UTF-8",
	CodeControlCharacter:     "write it as an escape",
	CodeLoneCarriageReturn:   "save the file with LF or CR LF",
	CodeUnexpectedCharacter:  "",
	CodeExpectedEquals:       "expected = or {",
	CodeExpectedBrace:        "expected { after the label",
	CodeExpectedValue:        "expected a value",
	CodeExpectedNewline:      "start a new line",
	CodeExpectedComma:        "expected , or ]",
	CodeDuplicateKey:         "remove one of them",
	CodeNameConflict:         "rename one of them",
	CodeDuplicateLabel:       "give this one another label",
	CodeUnclosedBlock:        "add a } for it",
	CodeUnexpectedClose:      "remove it",
	CodeUnterminatedString:   "add a \" at its end",
	CodeInvalidEscape:        "the escapes are",
	CodeInvalidUnicodeEscape: "such as \\u{e9}",
	CodeTextOpening:          "start the text on the next line",
	CodeTextIndent:           "indent it at least as far",
	CodeUnterminatedText:     "end it with \"\"\" on a line of its own",
	CodeUnterminatedList:     "add a ] at its end",
	CodeInvalidNumber:        "",
	CodeNumberOverflow:       "64-bit",
	CodeUnitNotAllowed:       "write the value in decimal",
	CodeTooDeep:              "more than 64 levels",
}

func TestEveryMessageSaysWhatToDo(t *testing.T) {
	files, err := filepath.Glob(filepath.Join(suite, "invalid", "*.meri"))
	if err != nil || len(files) == 0 {
		t.Fatalf("no invalid vectors found: %v", err)
	}
	for _, name := range files {
		src, err := os.ReadFile(name)
		if err != nil {
			t.Fatal(err)
		}
		_, err = Parse(filepath.Base(name), src)
		var e *Error
		if !errors.As(err, &e) {
			t.Fatalf("%s: %v", name, err)
		}
		advice, known := adviceByCode[e.Code]
		if !known {
			t.Errorf("%s: no advice listed for %s", name, e.Code)
			continue
		}
		if !strings.Contains(e.Msg, advice) {
			t.Errorf("%s: %s message %q does not contain %q", filepath.Base(name), e.Code, e.Msg, advice)
		}
		if strings.HasSuffix(e.Msg, ".") || e.Msg != strings.TrimSpace(e.Msg) {
			t.Errorf("%s: message %q should not end in a period or space", filepath.Base(name), e.Msg)
		}
		if first := e.Msg[:1]; first != strings.ToLower(first) {
			t.Errorf("%s: message %q should start in lower case, it follows the position", filepath.Base(name), e.Msg)
		}
	}
}

// Callers reuse their read buffers, so nothing in a document may point into
// the bytes it was parsed from.
func TestParsedValuesDoNotShareTheInput(t *testing.T) {
	src := []byte("name = \"Meridian\"\nmode = fast\nlabelled \"lbl\" { k = 1.5s }\nt = \"\"\"\n  text\n  \"\"\"\nl = [\"x\", y]\n")
	doc, err := Parse("in.meri", src)
	if err != nil {
		t.Fatal(err)
	}
	before := Dump(doc)
	for i := range src {
		src[i] = 'Z'
	}
	if after := Dump(doc); after != before {
		t.Fatalf("overwriting the input changed the document\nbefore:\n%s\nafter:\n%s", before, after)
	}
}

func TestParseIsSafeForConcurrentUse(t *testing.T) {
	src := benchDocument()[:64<<10]
	src = src[:strings.LastIndex(string(src), "\n}\n")+3]
	want, err := Parse("c.meri", src)
	if err != nil {
		t.Fatal(err)
	}
	wantDump := Dump(want)
	var wg sync.WaitGroup
	errs := make(chan string, 8)
	for range 8 {
		wg.Add(1)
		go func() {
			defer wg.Done()
			doc, err := Parse("c.meri", src)
			if err != nil {
				errs <- err.Error()
				return
			}
			if Dump(doc) != wantDump {
				errs <- "a concurrent parse produced a different document"
			}
		}()
	}
	wg.Wait()
	close(errs)
	for e := range errs {
		t.Error(e)
	}
}

func TestParseOfNilInput(t *testing.T) {
	doc, err := Parse("nil.meri", nil)
	if err != nil || doc == nil || len(doc.Items) != 0 {
		t.Fatalf("Parse(nil) = %+v, %v", doc, err)
	}
}

func TestQuoteEveryByte(t *testing.T) {
	named := map[byte]string{'\\': `\\`, '"': `\"`, '\n': `\n`, '\r': `\r`, '\t': `\t`, 0: `\0`}
	for b := range 128 {
		c := byte(b)
		want, ok := named[c]
		switch {
		case ok:
		case c < 0x20 || c == 0x7F:
			want = `\u{` + strconv.FormatInt(int64(c), 16) + `}`
		default:
			want = string(c)
		}
		if got := quote(string(c)); got != `"`+want+`"` {
			t.Errorf("quote(%q) = %s, want \"%s\"", c, got, want)
		}
	}
}

func TestLayersKeepTheFileOfEveryValue(t *testing.T) {
	parse := func(file, src string) *Block {
		doc, err := Parse(file, []byte(src))
		if err != nil {
			t.Fatal(err)
		}
		return doc
	}
	system := parse("system.meri", "qemu {\n  cpus = x\n  memory = y\n  accel = z\n}\n")
	project := parse("project.meri", "qemu {\n  memory = y\n}\n")
	local := parse("local.meri", "qemu {\n  accel = z\n}\n")
	q := Merge(Merge(system, project), local).Blocks("qemu")[0]
	for key, file := range map[string]string{"cpus": "system.meri", "memory": "project.meri", "accel": "local.meri"} {
		_, err := q.Int(key, 0)
		var e *Error
		if !errors.As(err, &e) || e.File != file {
			t.Errorf("%s: error %v, want it to name %s", key, err, file)
		}
	}
	if q.Pos.File != "system.meri" {
		t.Errorf("the merged block points at %s, want the base file", q.Pos.File)
	}
}

func TestMergeReachesIntoNestedBlocks(t *testing.T) {
	out := merged(t,
		"display \"DP-1\" {\n  mode { width = 1920\n height = 1080 }\n  scale = 100%\n}\n",
		"display \"DP-1\" {\n  mode { width = 2560 }\n}\ndisplay \"DP-2\" { scale = 150% }\n")
	want := `display#0 block "DP-1"
display#0.mode#0 block
display#0.mode#0.width int 2560
display#0.mode#0.height int 1080
display#0.scale quantity 100 %
display#1 block "DP-2"
display#1.scale quantity 150 %
`
	if got := Dump(out); got != want {
		t.Fatalf("got:\n%s\nwant:\n%s", got, want)
	}
}
