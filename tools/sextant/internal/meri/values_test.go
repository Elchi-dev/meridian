package meri

import (
	"reflect"
	"testing"
	"time"
)

// A settings file the way sextant will read it, through the getters only.
func TestReadingARealisticSettingsFile(t *testing.T) {
	src := `# Settings for sextant.

qemu {
    memory = 512MiB
    cpus = 4
    accel = kvm               # kvm, hvf, whpx or tcg
    firmware = "/usr/share/edk2/x64/OVMF_CODE.4m.fd"
    boot-timeout = 30s
}

stats {
    min-ratio = 1.5
    components = ["boot", "kernel", tools]
}

test {
    parallel = true
}
`
	local := "qemu {\n    memory = 2GiB\n}\n"
	base, err := Parse("sextant.meri", []byte(src))
	if err != nil {
		t.Fatal(err)
	}
	over, err := Parse("sextant.local.meri", []byte(local))
	if err != nil {
		t.Fatal(err)
	}
	doc := Merge(base, over)
	q := doc.Blocks("qemu")[0]
	mem, err1 := q.Size("memory", 0)
	cpus, err2 := q.Int("cpus", 1)
	accel, err3 := q.String("accel", "tcg")
	fw, err4 := q.String("firmware", "")
	wait, err5 := q.Duration("boot-timeout", time.Minute)
	gdb, err6 := q.Bool("gdb", false)
	s := doc.Blocks("stats")[0]
	ratio, err7 := s.Float("min-ratio", 0)
	parts, err8 := s.Strings("components", nil)
	parallel, err9 := doc.Blocks("test")[0].Bool("parallel", false)
	for i, err := range []error{err1, err2, err3, err4, err5, err6, err7, err8, err9} {
		if err != nil {
			t.Errorf("getter %d: %v", i+1, err)
		}
	}
	if mem != 2<<30 || cpus != 4 || accel != "kvm" || fw != "/usr/share/edk2/x64/OVMF_CODE.4m.fd" ||
		wait != 30*time.Second || gdb || ratio != 1.5 || !parallel ||
		!reflect.DeepEqual(parts, []string{"boot", "kernel", "tools"}) {
		t.Errorf("read mem=%d cpus=%d accel=%q fw=%q wait=%v gdb=%v ratio=%v parts=%v parallel=%v",
			mem, cpus, accel, fw, wait, gdb, ratio, parts, parallel)
	}
}

func TestLabelsWithEscapesAndErrors(t *testing.T) {
	doc := mustParse(t, "d \"a\\\"b\\u{e9}\" { }\nd plain-word { }\n")
	if _, ok := doc.LabelledBlock("d", "a\"b\u00e9"); !ok {
		t.Error("the escaped label was not decoded")
	}
	if _, ok := doc.LabelledBlock("d", "plain-word"); !ok {
		t.Error("the word label was not found")
	}
	tests := []struct {
		src       string
		line, col int
		code      string
	}{
		{"d \"open { }\n", 1, 3, CodeUnterminatedString},
		{"d \"bad\\q\" { }\n", 1, 7, CodeInvalidEscape},
		{"d \"x\" = 1\n", 1, 7, CodeExpectedBrace},
		{"d x y { }\n", 1, 5, CodeExpectedBrace},
		{"d \"x\"\n{ }\n", 1, 6, CodeExpectedBrace},
	}
	for _, tt := range tests {
		e := parseError(t, tt.src)
		if e.Line != tt.line || e.Col != tt.col || e.Code != tt.code {
			t.Errorf("%q: got %d:%d %s, want %d:%d %s", tt.src, e.Line, e.Col, e.Code, tt.line, tt.col, tt.code)
		}
	}
}

func TestWordsThatLookLikeBooleans(t *testing.T) {
	doc := mustParse(t, "a = trueish\nb = False\nc = TRUE\nd = true-ish\ne = yes\nfalse = true\n")
	for key, want := range map[string]string{"a": "trueish", "b": "False", "c": "TRUE", "d": "true-ish", "e": "yes"} {
		v, _ := doc.Get(key)
		if v.Kind != KindWord || v.Str != want {
			t.Errorf("%s = %+v, want the word %s", key, v, want)
		}
	}
	if v, _ := doc.Get("false"); v.Kind != KindBool || !v.Bool {
		t.Errorf("the key false holds %+v, want true", v)
	}
}

func TestTextEdgeCases(t *testing.T) {
	tests := []struct {
		name, src, want string
	}{
		{"trailing spaces on content lines stay", "t = \"\"\"\n  a  \n  \"\"\"\n", "a  "},
		{"deeper lines keep their extra indentation", "t = \"\"\"\n  a\n    b\n  \"\"\"\n", "a\n  b"},
		{"three quotes inside a line are text", "t = \"\"\"\n  say \"\"\" here\n  \"\"\"\n", "say \"\"\" here"},
		{"tabs as indentation", "t = \"\"\"\n\t\ta\n\t\t\"\"\"\n", "a"},
		{"spaces after the opening quotes", "t = \"\"\"   \n  a\n  \"\"\"\n", "a"},
		{"a closing brace after the text", "b { t = \"\"\"\n  a\n  \"\"\" }\n", "a"},
		{"an empty text in a list", "l = [\"\"\"\n\"\"\", 1]\n", ""},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			doc := mustParse(t, tt.src)
			var v Value
			switch {
			case len(doc.Blocks("b")) == 1:
				v, _ = doc.Blocks("b")[0].Get("t")
			case len(doc.Items) == 1 && doc.Items[0].Name == "l":
				v = doc.Items[0].Value.List[0]
			default:
				v, _ = doc.Get("t")
			}
			if v.Kind != KindString || v.Str != tt.want {
				t.Errorf("text = %+v, want %q", v, tt.want)
			}
		})
	}
}

func TestIntegerValuesInEveryBase(t *testing.T) {
	tests := []struct {
		literal string
		want    int64
	}{
		{"0", 0}, {"-0", 0}, {"42", 42}, {"-42", -42}, {"1_000", 1000},
		{"0x0", 0}, {"0xFF", 255}, {"0xff_ff", 65535}, {"-0x1", -1},
		{"0o0", 0}, {"0o777", 511}, {"-0o10", -8},
		{"0b0", 0}, {"0b1", 1}, {"0b1111_1111", 255}, {"-0b10", -2},
		{"9223372036854775807", 9223372036854775807},
		{"0x7FFF_FFFF_FFFF_FFFF", 9223372036854775807},
		{"-0b1000000000000000000000000000000000000000000000000000000000000000", -9223372036854775808},
	}
	for _, tt := range tests {
		doc := mustParse(t, "v = "+tt.literal+"\n")
		v, _ := doc.Get("v")
		if v.Kind != KindInt || v.Int != tt.want {
			t.Errorf("%s = %+v, want %d", tt.literal, v, tt.want)
		}
	}
}

func TestBlocksCannotBeListItems(t *testing.T) {
	e := parseError(t, "l = [ { } ]\n")
	if e.Code != CodeUnexpectedCharacter || e.Col != 7 {
		t.Fatalf("got %d:%d %s", e.Line, e.Col, e.Code)
	}
}
