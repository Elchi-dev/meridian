package srclex

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// show renders tokens as "kind:text" separated by spaces, which keeps the
// expectations below readable.
func show(toks []Token) string {
	parts := make([]string, len(toks))
	for i, t := range toks {
		parts[i] = t.Kind.String() + ":" + t.Text
	}
	return strings.Join(parts, " ")
}

func mustLex(t *testing.T, lang Lang, src string) []Token {
	t.Helper()
	toks, err := Lex(lang, []byte(src))
	if err != nil {
		t.Fatalf("Lex(%v, %q): %v", lang, src, err)
	}
	return toks
}

func TestLangOf(t *testing.T) {
	tests := []struct {
		path string
		want Lang
		ok   bool
	}{
		{"kernel/src/main.rs", Rust, true},
		{"tools/sextant/main.go", Go, true},
		{"boot/src/trampoline.s", Asm, true},
		{"boot/src/TRAMPOLINE.S", Asm, true},
		{"x/start.asm", Asm, true},
		{"kernel/link.ld", LinkerScript, true},
		{"README.md", 0, false},
		{"Cargo.toml", 0, false},
		{"Makefile", 0, false},
	}
	for _, tt := range tests {
		got, ok := LangOf(tt.path)
		if got != tt.want || ok != tt.ok {
			t.Errorf("LangOf(%q) = %v, %v; want %v, %v", tt.path, got, ok, tt.want, tt.ok)
		}
	}
}

func TestNames(t *testing.T) {
	langs := map[Lang]string{Rust: "Rust", Go: "Go", Asm: "assembly", LinkerScript: "linker script", Lang(9): "Lang(9)"}
	for l, want := range langs {
		if l.String() != want {
			t.Errorf("Lang %d prints as %q, want %q", uint8(l), l.String(), want)
		}
	}
	kinds := map[Kind]string{
		Ident: "ident", Number: "number", String: "string", Char: "char",
		Lifetime: "lifetime", Punct: "punct", Comment: "comment", Kind(0): "Kind(0)", Kind(8): "Kind(8)", Kind(42): "Kind(42)",
	}
	for k, want := range kinds {
		if k.String() != want {
			t.Errorf("Kind %d prints as %q, want %q", uint8(k), k.String(), want)
		}
	}
}

func TestRustTokens(t *testing.T) {
	tests := []struct {
		name, src, want string
	}{
		{"function", "fn main() {}", "ident:fn ident:main punct:( punct:) punct:{ punct:}"},
		{"paths and arrows", "a::b -> c => d", "ident:a punct::: ident:b punct:-> ident:c punct:=> ident:d"},
		{"comparison operators", "a <= b >= c == d != e", "ident:a punct:<= ident:b punct:>= ident:c punct:== ident:d punct:!= ident:e"},
		{"shift assignment beats shift", "x <<= 1; y >>= 2", "ident:x punct:<<= number:1 punct:; ident:y punct:>>= number:2"},
		{"logic", "a && !b || c", "ident:a punct:&& punct:! ident:b punct:|| ident:c"},
		{"compound assignment", "a += 1; b |= c; d ^= e", "ident:a punct:+= number:1 punct:; ident:b punct:|= ident:c punct:; ident:d punct:^= ident:e"},
		{"ranges are not floats", "0..10", "number:0 punct:.. number:10"},
		{"inclusive range", "0..=9", "number:0 punct:..= number:9"},
		{"float", "1.5 + 2.0e-3", "number:1.5 punct:+ number:2.0e-3"},
		{"exponent without sign", "6e23", "number:6e23"},
		{"method on integer", "1.max(2)", "number:1 punct:. ident:max punct:( number:2 punct:)"},
		{"suffixes and separators", "1_000u32 0xFF_u8 0b1010 0o777 2.5f64", "number:1_000u32 number:0xFF_u8 number:0b1010 number:0o777 number:2.5f64"},
		{"lifetimes", "fn f<'a>(x: &'a str) -> &'static str", "ident:fn ident:f punct:< lifetime:'a punct:> punct:( ident:x punct:: punct:& lifetime:'a ident:str punct:) punct:-> punct:& lifetime:'static ident:str"},
		{"loop label", "'outer: loop { break 'outer; }", "lifetime:'outer punct:: ident:loop punct:{ ident:break lifetime:'outer punct:; punct:}"},
		{"chars", `'a' '\n' '\'' '\u{1F600}' 'é'`, `char:'a' char:'\n' char:'\'' char:'\u{1F600}' char:'é'`},
		{"byte char and byte string", `b'x' b"bytes"`, `char:b'x' string:b"bytes"`},
		{"c string", `c"hello"`, `string:c"hello"`},
		{"string with escaped quote", `"say \"hi\""`, `string:"say \"hi\""`},
		{"raw string", `r"C:\path"`, `string:r"C:\path"`},
		{"raw string with hashes", `r#"a "quoted" word"#`, `string:r#"a "quoted" word"#`},
		{"raw string with two hashes", `r##"has "# inside"##`, `string:r##"has "# inside"##`},
		{"raw byte string", `br#"raw"#`, `string:br#"raw"#`},
		{"raw c string", `cr#"raw"# cr"x"`, `string:cr#"raw"# string:cr"x"`},
		{"slash right after the opener does not nest", "/*/ x */ */ y", "comment:/*/ x */ punct:* punct:/ ident:y"},
		{"raw identifier", "r#type", "ident:r#type"},
		{"identifier starting with r", "rest r2", "ident:rest ident:r2"},
		{"identifier starting with b", "bytes", "ident:bytes"},
		{"line comment", "x // note\ny", "ident:x comment:// note ident:y"},
		{"doc comment", "/// docs\nfn f(){}", "comment:/// docs ident:fn ident:f punct:( punct:) punct:{ punct:}"},
		{"nested block comment", "a /* x /* y */ z */ b", "ident:a comment:/* x /* y */ z */ ident:b"},
		{"macro and attribute", "#[cfg(test)] vec![1]", "punct:# punct:[ ident:cfg punct:( ident:test punct:) punct:] ident:vec punct:! punct:[ number:1 punct:]"},
		{"leading underscores", "let _unused = __x;", "ident:let ident:_unused punct:= ident:__x punct:;"},
		{"hex with exponent letters stays one token", "0x1e-5", "number:0x1e punct:- number:5"},
		{"unicode identifier", "let größe = 1;", "ident:let ident:größe punct:= number:1 punct:;"},
		{"question mark and at", "x? @ y", "ident:x punct:? punct:@ ident:y"},
		{"ellipsis", "a...b", "ident:a punct:... ident:b"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := show(mustLex(t, Rust, tt.src)); got != tt.want {
				t.Errorf("got  %s\nwant %s", got, tt.want)
			}
		})
	}
}

func TestGoTokens(t *testing.T) {
	tests := []struct {
		name, src, want string
	}{
		{"declaration", "x := 1", "ident:x punct::= number:1"},
		{"channel and increment", "ch <- v; i++; j--", "ident:ch punct:<- ident:v punct:; ident:i punct:++ punct:; ident:j punct:--"},
		{"and not", "a &^ b; c &^= d", "ident:a punct:&^ ident:b punct:; ident:c punct:&^= ident:d"},
		{"variadic", "f(xs...)", "ident:f punct:( ident:xs punct:... punct:)"},
		{"raw string", "`a\\nb`", "string:`a\\nb`"},
		{"rune", `'x' '\''`, `char:'x' char:'\''`},
		{"leading dot float", "x = .5", "ident:x punct:= number:.5"},
		{"imaginary", "2i", "number:2i"},
		{"block comments do not nest", "a /* x /* y */ b", "ident:a comment:/* x /* y */ ident:b"},
		{"comment ends inside a slash star slash", "/* a /*/ b", "comment:/* a /*/ ident:b"},
		{"shift", "a << 2 >> 1", "ident:a punct:<< number:2 punct:>> number:1"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := show(mustLex(t, Go, tt.src)); got != tt.want {
				t.Errorf("got  %s\nwant %s", got, tt.want)
			}
		})
	}
}

func TestAsmTokens(t *testing.T) {
	src := ".global start # entry\nstart: mov rax, [rsp + 8] // load\n/* block */ .asciz \"hi\"\n"
	want := "ident:.global ident:start comment:# entry ident:start punct:: ident:mov ident:rax punct:, punct:[ ident:rsp punct:+ ident:8 punct:] comment:// load comment:/* block */ ident:.asciz string:\"hi\""
	if got := show(mustLex(t, Asm, src)); got != want {
		t.Errorf("got  %s\nwant %s", got, want)
	}
}

func TestLinkerScriptTokens(t *testing.T) {
	src := "/* where the kernel lives */\nSECTIONS { . = 0xffffffff80000000; .text : { *(.text*) } }"
	want := "comment:/* where the kernel lives */ ident:SECTIONS punct:{ ident:. punct:= ident:0xffffffff80000000 punct:; ident:.text punct:: punct:{ punct:* punct:( ident:.text punct:* punct:) punct:} punct:}"
	if got := show(mustLex(t, LinkerScript, src)); got != want {
		t.Errorf("got  %s\nwant %s", got, want)
	}
}

func TestLinkerScriptHasNoHashComments(t *testing.T) {
	toks := mustLex(t, LinkerScript, "# not a comment")
	if toks[0].Kind == Comment {
		t.Fatalf("GNU ld only knows /* */ comments, got %s", show(toks))
	}
}

func TestPositions(t *testing.T) {
	src := "let a = 1;\n/* two\nlines */\nlet s = r#\"x\ny\"#;"
	toks := mustLex(t, Rust, src)
	find := func(text string) Token {
		for _, tok := range toks {
			if tok.Text == text {
				return tok
			}
		}
		t.Fatalf("no token %q in %s", text, show(toks))
		return Token{}
	}
	if tok := find("a"); tok.Line != 1 || tok.EndLine != 1 || tok.Start != 4 || tok.End != 5 {
		t.Errorf("a: %+v", tok)
	}
	if tok := find("/* two\nlines */"); tok.Line != 2 || tok.EndLine != 3 {
		t.Errorf("block comment spans lines %d to %d, want 2 to 3", tok.Line, tok.EndLine)
	}
	if tok := find("r#\"x\ny\"#"); tok.Line != 4 || tok.EndLine != 5 || src[tok.Start:tok.End] != tok.Text {
		t.Errorf("raw string: %+v", tok)
	}
	if tok := find(";"); tok.Line != 1 {
		t.Errorf("first semicolon on line %d", tok.Line)
	}
	last := toks[len(toks)-1]
	if last.Text != ";" || last.Line != 5 || last.End != len(src) {
		t.Errorf("last token: %+v", last)
	}
}

func TestEscapedNewlineInRustStringKeepsLineCount(t *testing.T) {
	toks := mustLex(t, Rust, "\"a\\\nb\" x")
	if toks[1].Text != "x" || toks[1].Line != 2 {
		t.Fatalf("x should be on line 2: %+v", toks)
	}
	if toks[0].EndLine != 2 {
		t.Fatalf("string should end on line 2: %+v", toks[0])
	}
}

func TestCRLF(t *testing.T) {
	toks := mustLex(t, Go, "a\r\nb // c\r\nd")
	if show(toks) != "ident:a ident:b comment:// c\r ident:d" {
		t.Fatalf("got %s", show(toks))
	}
	if toks[3].Line != 3 {
		t.Fatalf("d on line %d, want 3", toks[3].Line)
	}
}

func TestErrors(t *testing.T) {
	tests := []struct {
		name      string
		lang      Lang
		src       string
		line, col int
		msg       string
	}{
		{"open block comment", Rust, "x\n  /* never", 2, 3, "block comment is never closed"},
		{"open nested comment", Rust, "/* a /* b */", 1, 1, "block comment is never closed"},
		{"open string", Rust, `let s = "abc`, 1, 9, "string literal is never closed"},
		{"backslash at the very end", Rust, `"abc\`, 1, 1, "string literal is never closed"},
		{"open raw string", Rust, `r#"abc"`, 1, 1, "raw string is never closed"},
		{"raw string without quote", Rust, `r##x`, 1, 1, "raw string needs a quote after its hashes"},
		{"stray quote", Rust, "x = ' ;", 1, 5, "a quote here starts neither a character nor a lifetime"},
		{"byte char across lines", Rust, "b'x\n'", 1, 1, "char literal runs into the end of the line"},
		{"escaped newline then end", Rust, "'\\\n", 1, 1, "char literal is never closed"},
		{"go string across lines", Go, "s := \"a\nb\"", 1, 6, "string literal runs into the end of the line"},
		{"go raw string", Go, "`never", 1, 1, "raw string is never closed"},
		{"go block comment", Go, "/* x", 1, 1, "block comment is never closed"},
		{"asm string", Asm, ".ascii \"x", 1, 8, "string literal is never closed"},
		{"ld comment", LinkerScript, "/*", 1, 1, "block comment is never closed"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			_, err := Lex(tt.lang, []byte(tt.src))
			var lexErr *Error
			if !errors.As(err, &lexErr) {
				t.Fatalf("want an *Error, got %v", err)
			}
			if lexErr.Line != tt.line || lexErr.Col != tt.col || lexErr.Msg != tt.msg {
				t.Fatalf("got %d:%d %q, want %d:%d %q", lexErr.Line, lexErr.Col, lexErr.Msg, tt.line, tt.col, tt.msg)
			}
			if want := fmt.Sprintf("%d:%d: %s", tt.line, tt.col, tt.msg); err.Error() != want {
				t.Fatalf("Error() = %q, want %q", err.Error(), want)
			}
		})
	}
}

func TestErrorString(t *testing.T) {
	err := &Error{Line: 3, Col: 7, Msg: "broken"}
	if err.Error() != "3:7: broken" {
		t.Fatalf("got %q", err.Error())
	}
}

func TestUnknownLanguage(t *testing.T) {
	if _, err := Lex(Lang(0), []byte("x")); err == nil || err.Error() != "srclex: unknown language Lang(0)" {
		t.Fatalf("got %v", err)
	}
}

func TestEmptyAndWhitespaceSources(t *testing.T) {
	for _, lang := range []Lang{Rust, Go, Asm, LinkerScript} {
		for _, src := range []string{"", " \t\r\n\f\v\n"} {
			toks, err := Lex(lang, []byte(src))
			if err != nil || len(toks) != 0 {
				t.Errorf("%v %q: %s, %v", lang, src, show(toks), err)
			}
		}
	}
}

func TestLineCount(t *testing.T) {
	tests := []struct {
		src  string
		want int
	}{
		{"", 0},
		{"a", 1},
		{"a\n", 1},
		{"a\nb", 2},
		{"\n\n", 2},
		{"a\n\nb\n", 3},
	}
	for _, tt := range tests {
		if got := LineCount([]byte(tt.src)); got != tt.want {
			t.Errorf("LineCount(%q) = %d, want %d", tt.src, got, tt.want)
		}
	}
}

func TestClassify(t *testing.T) {
	src := "// header\n\nfn main() { // trailing\n    let s = \"multi\nline\";\n    /* only\n       comment */\n}\n"
	toks := mustLex(t, Rust, src)
	got := Classify(LineCount([]byte(src)), toks)
	want := []LineKind{CommentOnly, Blank, Code, Code, Code, CommentOnly, CommentOnly, Code}
	if len(got) != len(want) {
		t.Fatalf("got %d lines, want %d", len(got), len(want))
	}
	for i := range want {
		if got[i] != want[i] {
			t.Errorf("line %d: got %d, want %d", i+1, got[i], want[i])
		}
	}
}

func TestClassifyCodeWinsOverComment(t *testing.T) {
	src := "/* a */ x /* b */"
	got := Classify(1, mustLex(t, Go, src))
	if got[0] != Code {
		t.Fatalf("got %d, want Code", got[0])
	}
}

func TestClassifyIgnoresTokensPastTheLastLine(t *testing.T) {
	toks := []Token{{Kind: Ident, Line: 1, EndLine: 3}}
	got := Classify(2, toks)
	if len(got) != 2 || got[0] != Code || got[1] != Code {
		t.Fatalf("got %v", got)
	}
}

// A missing entry in an operator table would silently split that operator
// into two tokens, and the mutation tester would then mutate the halves.
func TestEveryOperatorIsOneToken(t *testing.T) {
	for _, tc := range []struct {
		lang  Lang
		table []string
	}{{Rust, rustPuncts}, {Go, goPuncts}} {
		for _, op := range tc.table {
			toks := mustLex(t, tc.lang, "a "+op+" b")
			if len(toks) != 3 || toks[1].Kind != Punct || toks[1].Text != op {
				t.Errorf("%v %q lexes as %s", tc.lang, op, show(toks))
			}
		}
	}
}

func TestLongerOperatorsWin(t *testing.T) {
	for _, tc := range []struct {
		lang      Lang
		src, want string
	}{
		{Rust, "a<<=b", "ident:a punct:<<= ident:b"},
		{Rust, "a..=b", "ident:a punct:..= ident:b"},
		{Rust, "a...b", "ident:a punct:... ident:b"},
		{Rust, "a<-b", "ident:a punct:< punct:- ident:b"},
		{Go, "a&^=b", "ident:a punct:&^= ident:b"},
		{Go, "a<-b", "ident:a punct:<- ident:b"},
		{Go, "a:=b", "ident:a punct::= ident:b"},
		{Go, "a::b", "ident:a punct:: punct:: ident:b"},
	} {
		if got := show(mustLex(t, tc.lang, tc.src)); got != tc.want {
			t.Errorf("%v %q: got %s, want %s", tc.lang, tc.src, got, tc.want)
		}
	}
}

// checkTiling verifies the invariants every caller relies on: tokens come in
// order, their text is exactly the source between Start and End, only
// whitespace lies between them, and the line numbers match the source.
func checkTiling(t *testing.T, name string, src []byte, toks []Token) {
	t.Helper()
	prev := 0
	for i, tok := range toks {
		if tok.Start < prev || tok.End <= tok.Start || tok.End > len(src) {
			t.Fatalf("%s: token %d %q has bad bounds %d..%d after %d", name, i, tok.Text, tok.Start, tok.End, prev)
		}
		if string(src[tok.Start:tok.End]) != tok.Text {
			t.Fatalf("%s: token %d text %q does not match the source %q", name, i, tok.Text, src[tok.Start:tok.End])
		}
		if gap := strings.TrimSpace(string(src[prev:tok.Start])); gap != "" {
			t.Fatalf("%s: non-space %q skipped before token %d %q", name, gap, i, tok.Text)
		}
		if want := 1 + strings.Count(string(src[:tok.Start]), "\n"); tok.Line != want {
			t.Fatalf("%s: token %d %q on line %d, want %d", name, i, tok.Text, tok.Line, want)
		}
		if want := 1 + strings.Count(string(src[:tok.End-1]), "\n"); tok.EndLine != want {
			t.Fatalf("%s: token %d %q ends on line %d, want %d", name, i, tok.Text, tok.EndLine, want)
		}
		prev = tok.End
	}
	if rest := strings.TrimSpace(string(src[prev:])); rest != "" {
		t.Fatalf("%s: trailing %q was never tokenized", name, rest)
	}
}

const realisticRust = `//! Frame allocator.
use core::ops::Range;

/// One physical frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame(u64);

impl<'a> Iterator for Frames<'a> {
    type Item = Frame;

    fn next(&mut self) -> Option<Self::Item> {
        'scan: while self.at < self.end {
            let bit = (self.map[(self.at / 64) as usize] >> (self.at % 64)) & 1;
            self.at += 1;
            if bit == 0 {
                continue 'scan;
            }
            return Some(Frame(self.at * 4096));
        }
        None
    }
}

const MAGIC: &[u8; 4] = b"MERI";
static BANNER: &str = r#"Meridian "kernel""#;
const NEWLINE: char = '\n';

fn sum(xs: &[u32]) -> u32 {
    xs.iter().fold(0_u32, |acc, x| acc.wrapping_add(*x))
}

unsafe fn outb(port: u16, value: u8) {
    /* SAFETY: callers pass a port they own. /* nested */ */
    core::arch::asm!("out dx, al", in("dx") port, in("al") value);
}
`

const realisticGo = "package main\n\nimport (\n\t\"fmt\"\n\t\"os\"\n)\n\n/* usage: tool [-v] */\nfunc main() {\n\tch := make(chan int, 1)\n\tch <- 0x1F &^ 0b11\n\tv := <-ch\n\ts := `raw\nstring`\n\tr := '\\''\n\tfor i := 0; i < len(os.Args); i++ {\n\t\tv += i\n\t}\n\tfmt.Println(v, s, r, 1.5e-3, .25, 3i)\n}\n"

func TestTilingOnRealisticSources(t *testing.T) {
	for _, tc := range []struct {
		name string
		lang Lang
		src  string
	}{
		{"rust", Rust, realisticRust},
		{"go", Go, realisticGo},
		{"asm", Asm, ".intel_syntax noprefix\n.global _start\n_start:\n    mov rsp, offset stack_top  # set up a stack\n    call kmain\n1:  hlt\n    jmp 1b\n"},
		{"ld", LinkerScript, "ENTRY(_start)\nSECTIONS\n{\n    . = 0xffffffff80000000;\n    .text : ALIGN(4K) { *(.text .text.*) }\n    /DISCARD/ : { *(.comment) }\n}\n"},
	} {
		toks := mustLex(t, tc.lang, tc.src)
		checkTiling(t, tc.name, []byte(tc.src), toks)
	}
}

func TestRealisticRustKinds(t *testing.T) {
	counts := map[Kind]int{}
	for _, tok := range mustLex(t, Rust, realisticRust) {
		counts[tok.Kind]++
	}
	want := map[Kind]int{Lifetime: 4, Char: 1, Comment: 3, String: 5}
	for kind, n := range want {
		if counts[kind] != n {
			t.Errorf("%v tokens: %d, want %d", kind, counts[kind], n)
		}
	}
}

// The Go files of sextant itself are a free, realistic corpus that grows
// with the project.
func TestTilingOnSextantsOwnSources(t *testing.T) {
	paths, err := filepath.Glob("../*/*.go")
	if err != nil || len(paths) < 6 {
		t.Fatalf("expected sextant's sources next to this package, found %d (%v)", len(paths), err)
	}
	for _, p := range paths {
		src, err := os.ReadFile(p)
		if err != nil {
			t.Fatal(err)
		}
		toks, err := Lex(Go, src)
		if err != nil {
			t.Fatalf("%s: %v", p, err)
		}
		checkTiling(t, p, src, toks)
	}
}

func TestEdgeCases(t *testing.T) {
	tests := []struct {
		name string
		lang Lang
		src  string
		want string
	}{
		{"anonymous lifetime", Rust, "impl Foo<'_>", "ident:impl ident:Foo punct:< lifetime:'_ punct:>"},
		{"byte string escapes", Rust, `b"\x00\""`, `string:b"\x00\""`},
		{"raw string marker inside a string", Rust, `"r#" x`, `string:"r#" ident:x`},
		{"br and cr alone are identifiers", Rust, "br cr", "ident:br ident:cr"},
		{"b and c alone are identifiers", Rust, "b c", "ident:b ident:c"},
		{"shebang-like inner attribute", Rust, "#![no_std]", "punct:# punct:! punct:[ ident:no_std punct:]"},
		{"closure bars", Rust, "|a, b| a | b", "punct:| ident:a punct:, ident:b punct:| ident:a punct:| ident:b"},
		{"turbofish", Rust, "f::<u8>()", "ident:f punct::: punct:< ident:u8 punct:> punct:( punct:)"},
		{"double generic close", Rust, "Vec<Vec<u8>>", "ident:Vec punct:< ident:Vec punct:< ident:u8 punct:>>"},
		{"tuple index", Rust, "t.0", "ident:t punct:. number:0"},
		{"symbols outside ASCII", Rust, "a § b", "ident:a punct:§ ident:b"},
		{"dollar in macro", Rust, "$x:expr", "punct:$ ident:x punct:: ident:expr"},
		{"go separators", Go, "1_000_000", "number:1_000_000"},
		{"go string with escapes", Go, `"tab\t\"q\""`, `string:"tab\t\"q\""`},
		{"go labels", Go, "outer:\n\tbreak outer", "ident:outer punct:: ident:break ident:outer"},
		{"asm multi-line comment", Asm, "nop /* one\ntwo */ ret", "ident:nop comment:/* one\ntwo */ ident:ret"},
		{"asm local label", Asm, ".Lloop: dec ecx", "ident:.Lloop punct:: ident:dec ident:ecx"},
		{"asm at symbol", Asm, "call foo@PLT", "ident:call ident:foo@PLT"},
		{"ld string", LinkerScript, `INCLUDE "common.ld"`, `ident:INCLUDE string:"common.ld"`},
		{"ld arithmetic", LinkerScript, ". = ALIGN(. + 8, 4096);", "ident:. punct:= ident:ALIGN punct:( ident:. punct:+ ident:8 punct:, ident:4096 punct:) punct:;"},
	}
	for _, tt := range tests {
		if got := show(mustLex(t, tt.lang, tt.src)); got != tt.want {
			t.Errorf("%s:\ngot  %s\nwant %s", tt.name, got, tt.want)
		}
	}
}

// Columns count bytes, not characters: editors and compilers report byte
// columns too, and sextant only uses them to point at a spot.
func TestErrorColumnsCountBytes(t *testing.T) {
	tests := []struct {
		src string
		col int
	}{
		{"\t\"open", 2},
		{"é \"open", 4},
		{"x\n\t\t\"open", 3},
	}
	for _, tt := range tests {
		_, err := Lex(Rust, []byte(tt.src))
		var lexErr *Error
		if !errors.As(err, &lexErr) || lexErr.Col != tt.col {
			t.Errorf("%q: got %v, want column %d", tt.src, err, tt.col)
		}
	}
}

func TestTokensKeepTheirOwnCopyOfTheText(t *testing.T) {
	src := []byte("let x = 1;")
	toks, err := Lex(Rust, src)
	if err != nil {
		t.Fatal(err)
	}
	ref := toks[1].Text
	src[4] = 'y'
	if toks[1].Text != ref || ref != "x" {
		t.Fatalf("token text changed with the source buffer: %q", toks[1].Text)
	}
}

func TestMoreLiterals(t *testing.T) {
	tests := []struct {
		name string
		lang Lang
		src  string
		want string
	}{
		{"byte char escapes", Rust, `b'\'' b'\\' b'\x7f'`, `char:b'\'' char:b'\\' char:b'\x7f'`},
		{"raw string with three hashes", Rust, `r###"a "## b"### x`, `string:r###"a "## b"### ident:x`},
		{"raw string spanning lines", Rust, "r\"a\n\nb\"", "string:r\"a\n\nb\""},
		{"asm string escape", Asm, `.ascii "a\"b"`, `ident:.ascii string:"a\"b"`},
		{"ld multi-line comment", LinkerScript, "/* a\n * b\n */ ENTRY(x)", "comment:/* a\n * b\n */ ident:ENTRY punct:( ident:x punct:)"},
		{"go octal and binary", Go, "0o17 0b101 017", "number:0o17 number:0b101 number:017"},
		{"go float forms", Go, "1. 1.5 1e3 1.5E+3", "number:1 punct:. number:1.5 number:1e3 number:1.5E+3"},
	}
	for _, tt := range tests {
		if got := show(mustLex(t, tt.lang, tt.src)); got != tt.want {
			t.Errorf("%s:\ngot  %s\nwant %s", tt.name, got, tt.want)
		}
	}
}

func TestGoRuneAcrossLines(t *testing.T) {
	_, err := Lex(Go, []byte("r := 'a\n'"))
	var lexErr *Error
	if !errors.As(err, &lexErr) || lexErr.Msg != "char literal runs into the end of the line" || lexErr.Col != 6 {
		t.Fatalf("got %v", err)
	}
}

func TestClassifyAssemblyAndLinkerScripts(t *testing.T) {
	tests := []struct {
		lang Lang
		src  string
		want []LineKind
	}{
		{Asm, "# entry point\n_start:\n\n    // stack\n    mov rsp, stack_top\n", []LineKind{CommentOnly, Code, Blank, CommentOnly, Code}},
		{LinkerScript, "/* kernel\n   layout */\nENTRY(_start)\n\nSECTIONS { }\n", []LineKind{CommentOnly, CommentOnly, Code, Blank, Code}},
		{Go, "package a\r\n\r\n// x\r\nvar b = 1\r\n", []LineKind{Code, Blank, CommentOnly, Code}},
	}
	for _, tt := range tests {
		got := Classify(LineCount([]byte(tt.src)), mustLex(t, tt.lang, tt.src))
		if len(got) != len(tt.want) {
			t.Fatalf("%v: %d lines, want %d", tt.lang, len(got), len(tt.want))
		}
		for i := range tt.want {
			if got[i] != tt.want[i] {
				t.Errorf("%v line %d: got %d, want %d", tt.lang, i+1, got[i], tt.want[i])
			}
		}
	}
}

// Cases sextant mutate showed were missing.
func TestBoundaries(t *testing.T) {
	tests := []struct {
		name string
		lang Lang
		src  string
		want string
	}{
		{"rust string over two lines", Rust, "\"a\nb\" x", "string:\"a\nb\" ident:x"},
		{"byte string over two lines", Rust, "b\"a\nb\" x", "string:b\"a\nb\" ident:x"},
		{"C string over two lines", Rust, "c\"a\nb\" x", "string:c\"a\nb\" ident:x"},
		{"line comment at the very end", Rust, "x // end", "ident:x comment:// end"},
		{"lone r is an identifier", Rust, "r + 1", "ident:r punct:+ number:1"},
		{"exponent with sign", Rust, "1e-3 2E+4", "number:1e-3 number:2E+4"},
		{"lifetime at the very end", Rust, "&'a", "punct:& lifetime:'a"},
		{"identifier at the very end", Go, "x := y", "ident:x punct::= ident:y"},
		{"asm comments do not nest", Asm, "/* a /* b */ x */", "comment:/* a /* b */ ident:x punct:* punct:/"},
		{"ld comments do not nest", LinkerScript, "/* a /* b */ x */", "comment:/* a /* b */ ident:x punct:* punct:/"},
		{"asm division", Asm, "x / y", "ident:x punct:/ ident:y"},
		{"ld division", LinkerScript, "x / y", "ident:x punct:/ ident:y"},
	}
	for _, tt := range tests {
		if got := show(mustLex(t, tt.lang, tt.src)); got != tt.want {
			t.Errorf("%s:\ngot  %s\nwant %s", tt.name, got, tt.want)
		}
	}
}

func TestBoundaryErrors(t *testing.T) {
	tests := []struct {
		name string
		lang Lang
		src  string
		msg  string
	}{
		{"char escape then a line break", Rust, "'\\a\n'", "char literal runs into the end of the line"},
		{"lone quote at the very end", Rust, "x '", "a quote here starts neither a character nor a lifetime"},
		{"asm string over two lines", Asm, "\"a\nb\"", "string literal runs into the end of the line"},
		{"ld string over two lines", LinkerScript, "\"a\nb\"", "string literal runs into the end of the line"},
	}
	for _, tt := range tests {
		_, err := Lex(tt.lang, []byte(tt.src))
		var lexErr *Error
		if !errors.As(err, &lexErr) || lexErr.Msg != tt.msg {
			t.Errorf("%s: got %v, want %q", tt.name, err, tt.msg)
		}
	}
}
