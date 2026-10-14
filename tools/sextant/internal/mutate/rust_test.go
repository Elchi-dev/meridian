package mutate

import (
	"strings"
	"testing"

	"github.com/Elchi-dev/meridian/tools/sextant/internal/srclex"
)

func rustMutants(t *testing.T, src string) []Mutant {
	t.Helper()
	ms, err := RustMutants("k/src/lib.rs", []byte(src), func(int) bool { return false })
	if err != nil {
		t.Fatalf("RustMutants: %v", err)
	}
	return ms
}

func rustDescs(t *testing.T, src string) string {
	t.Helper()
	var out []string
	for _, m := range rustMutants(t, src) {
		out = append(out, m.Desc)
	}
	return strings.Join(out, ",")
}

func TestRustBinaryOperators(t *testing.T) {
	got := rustDescs(t, "const X: bool = a < b && c != d || e >= f;\n")
	want := "< to <=,< to >=,&& to ||,!= to ==,|| to &&,>= to <,>= to >"
	if got != want {
		t.Fatalf("got  %s\nwant %s", got, want)
	}
}

func TestRustArithmeticAndBits(t *testing.T) {
	got := rustDescs(t, "const X: u32 = (a as u32) << 16 | b * 2 + c - d / e % f & g >> 1;\n")
	want := "<< to >>,| to &,* to /,+ to -,- to +,/ to *,% to *,& to |,>> to <<"
	if got != want {
		t.Fatalf("got  %s\nwant %s", got, want)
	}
}

func TestRustCompoundAssignments(t *testing.T) {
	got := rustDescs(t, "const X: () = {\n    x += 1;\n    self.y -= 2;\n    buf[i] |= 4;\n    z <<= 1;\n    w = 3;\n};\n")
	want := "+= to -=,-= to +=,|= to &=,<<= to >>="
	if got != want {
		t.Fatalf("got  %s\nwant %s", got, want)
	}
}

func TestRustTypesReferencesAndClosuresAreNotOperators(t *testing.T) {
	for _, src := range []string{
		"static V: Vec<Vec<u8>> = Vec::new();\n",
		"const R: &&str = &&\"x\";\n",
		"const F: fn() = || {};\n",
		"const G: i32 = -1;\n",
		"const H: u8 = *p;\n",
		"const C: usize = items.iter().map(|x| x.len()).count();\n",
		"const M: () = run(move || work());\n",
		"const D: &[u8] = &buf[..];\n",
		"fn f() -> impl Iterator<Item = char> + '_ {\n    iter()\n}\n",
		"const T: u8 = if !ok { 1 } else { 2 };\n",
	} {
		for _, m := range rustMutants(t, src) {
			if !strings.HasPrefix(m.Desc, "body to") && m.Desc != "! to nothing" {
				t.Errorf("%q: unexpected mutant %s at %d:%d", src, m.Desc, m.Line, m.Col)
			}
		}
	}
}

func TestRustMatchArmBarsAreSkipped(t *testing.T) {
	got := rustDescs(t, "const X: u8 = match k {\n    A | B => a | b,\n    _ => 0,\n};\n")
	if got != "| to &" {
		t.Fatalf("only the | after => is an operator, got %s", got)
	}
}

func TestRustUnaryNot(t *testing.T) {
	src := "fn f() {\n    if !ok && !self.done() {\n        assert!(x);\n    }\n}\n"
	ms := rustMutants(t, src)
	var nots []string
	for _, m := range ms {
		if m.Desc == "! to nothing" {
			nots = append(nots, itoa(m.Line)+":"+itoa(m.Col))
		}
	}
	if strings.Join(nots, " ") != "2:8 2:15" {
		t.Fatalf("! mutants at %v, want 2:8 2:15", nots)
	}
	var first Mutant
	for _, m := range ms {
		if m.Desc == "! to nothing" {
			first = m
			break
		}
	}
	if got := string(first.Apply([]byte(src))); !strings.Contains(got, "if ok && !self.done()") {
		t.Fatalf("got %q", got)
	}
}

func TestRustNeverTypeAndMacrosKeepTheirBang(t *testing.T) {
	for _, src := range []string{
		"fn halt() -> ! {\n    loop {}\n}\n",
		"macro_rules! m {\n    () => {};\n}\n",
		"const S: &str = concat!(\"a\", \"b\");\n",
	} {
		for _, m := range rustMutants(t, src) {
			if m.Desc == "! to nothing" {
				t.Errorf("%q: ! at %d:%d is not a negation", src, m.Line, m.Col)
			}
		}
	}
}

func TestRustBooleans(t *testing.T) {
	got := rustDescs(t, "const A: bool = true;\nconst B: bool = false;\nconst C: bool = truth;\n")
	if got != "true to false,false to true" {
		t.Fatalf("got %s", got)
	}
}

func TestRustAttributesAreLeftAlone(t *testing.T) {
	src := "#[cfg(not(feature = \"x\"))]\n#![allow(dead_code)]\n#[derive(A | B)]\nconst X: u8 = 1 + 2;\n"
	if got := rustDescs(t, src); got != "+ to -" {
		t.Fatalf("got %s", got)
	}
}

func TestRustTestLinesAreSkipped(t *testing.T) {
	src := "const A: u8 = 1 + 2;\nconst B: u8 = 3 + 4;\nconst C: u8 = 5 + 6;\n"
	ms, err := RustMutants("k/src/lib.rs", []byte(src), func(line int) bool { return line == 2 })
	if err != nil {
		t.Fatal(err)
	}
	if len(ms) != 2 || ms[0].Line != 1 || ms[1].Line != 3 {
		t.Fatalf("%v", descs(ms))
	}
}

func TestRustOperatorsInStringsAndCommentsAreText(t *testing.T) {
	src := "// a < b\nconst S: &str = \"a < b && true\"; /* x + y */\nconst R: &str = r#\"1 + 2\"#;\n"
	if got := rustDescs(t, src); got != "" {
		t.Fatalf("got %s", got)
	}
}

func TestRustFunctionBodies(t *testing.T) {
	src := `fn unit() {
    work();
}

pub fn yes(x: u8) -> bool {
    x > 1
}

fn fallible() -> Result<(), Status> {
    check()?;
    Ok(())
}

fn number() -> u32 {
    4
}

fn result_value() -> Result<u8, E> {
    Ok(1)
}

fn empty() {}

fn generic<F: Fn(u8) -> bool, T>(f: F) -> bool where T: Into<Vec<u8>> {
    f(1)
}

fn tuple() -> () {
    run();
}

fn array() -> [u8; 2] {
    [1, 2]
}

fn deep<T: Into<Vec<u8>>>(x: T) -> bool {
    x.into().is_empty()
}

fn alias() -> Result<()> {
    check()
}
`
	var got []string
	for _, m := range rustMutants(t, src) {
		if strings.HasPrefix(m.Desc, "body to") {
			got = append(got, itoa(m.Line)+" "+m.Desc)
		}
	}
	want := []string{
		"1 body to {}",
		"5 body to { false }",
		"5 body to { true }",
		"9 body to { Ok(()) }",
		"24 body to { false }",
		"24 body to { true }",
		"28 body to {}",
		"36 body to { false }",
		"36 body to { true }",
		"40 body to { Ok(()) }",
	}
	if strings.Join(got, "\n") != strings.Join(want, "\n") {
		t.Fatalf("got\n%s\nwant\n%s", strings.Join(got, "\n"), strings.Join(want, "\n"))
	}
}

func TestRustBodyReplacementText(t *testing.T) {
	src := "fn yes() -> bool {\n    x > 1\n}\n"
	for _, m := range rustMutants(t, src) {
		if m.Desc == "body to { false }" {
			if got := string(m.Apply([]byte(src))); got != "fn yes() -> bool { false }\n" {
				t.Fatalf("got %q", got)
			}
			return
		}
	}
	t.Fatal("no body mutant")
}

func TestRustFunctionsWithoutBodies(t *testing.T) {
	src := "trait T {\n    fn f(&self) -> bool;\n}\nextern \"C\" {\n    fn g();\n}\nstruct S {\n    stall: unsafe extern \"efiapi\" fn(usize) -> Status,\n}\n"
	if got := rustDescs(t, src); got != "" {
		t.Fatalf("got %s", got)
	}
}

func TestRustUnfinishedFunctions(t *testing.T) {
	for _, src := range []string{"fn", "fn f", "fn f<T", "fn f(x: u8", "fn f(x: u8) -> bool", "fn f() {"} {
		if _, err := RustMutants("k/src/lib.rs", []byte(src), func(int) bool { return false }); err != nil {
			t.Errorf("%q: %v", src, err)
		}
	}
}

func TestRustLexErrors(t *testing.T) {
	if _, err := RustMutants("k/src/lib.rs", []byte("\"open"), func(int) bool { return false }); err == nil {
		t.Fatal("an unterminated string must be an error")
	}
}

func TestRustMutantMetadata(t *testing.T) {
	ms := rustMutants(t, "fn f(a: u8) -> u8 {\n        a + 1\n}\n")
	m := ms[0]
	if m.Line != 2 || m.Col != 11 || m.LineText != "a + 1" || m.Path != "k/src/lib.rs" {
		t.Fatalf("%+v", m)
	}
}

func TestAttributesHelper(t *testing.T) {
	ms, _ := RustMutants("x.rs", []byte("#[a]\n#\nconst X: u8 = 1 + 1;\n#["), func(int) bool { return false })
	if len(ms) != 1 {
		t.Fatalf("a lone # and an open attribute at the end: %v", descs(ms))
	}
}

func TestEndsOperand(t *testing.T) {
	src := "a 1 \"s\" 'c' ) ] ? } ( return as move 'a +"
	toks := lexRust(t, src)
	want := []bool{true, true, true, true, true, true, true, false, false, false, false, false, false, false}
	for i, tok := range toks {
		if endsOperand(tok) != want[i] {
			t.Errorf("%q: endsOperand = %v, want %v", tok.Text, !want[i], want[i])
		}
	}
}

func lexRust(t *testing.T, src string) []srclex.Token {
	t.Helper()
	toks, err := srclex.Lex(srclex.Rust, []byte(src))
	if err != nil {
		t.Fatal(err)
	}
	return toks
}

func TestRustLineBrokenOperatorAfterAComment(t *testing.T) {
	got := rustDescs(t, "const X: u8 = aaaa // the first part\n    + bbbb;\n")
	if got != "+ to -" {
		t.Fatalf("comments between the operands must not hide the operator, got %q", got)
	}
}

func TestRustOperatorsNeedSpaceOnBothSides(t *testing.T) {
	if got := rustDescs(t, "const X: u8 = a -b;\nconst Y: u8 = a- b;\n"); got != "" {
		t.Fatalf("got %s", got)
	}
}

func TestRustNeverTypeBeforeANewline(t *testing.T) {
	for _, m := range rustMutants(t, "fn halt() -> !\n{\n    loop {}\n}\n") {
		if m.Desc == "! to nothing" {
			t.Fatalf("the never type at %d:%d is not a negation", m.Line, m.Col)
		}
	}
}

func TestRustFunctionPointerTypesDoNotStartFunctions(t *testing.T) {
	src := "struct S {\n    f: fn(u8) -> bool,\n}\nfn g() -> bool {\n    ready()\n}\n"
	if got := rustDescs(t, src); got != "body to { false },body to { true }" {
		t.Fatalf("got %s", got)
	}
}

func TestRustUnterminatedBodiesGetNoBodyMutant(t *testing.T) {
	for _, m := range rustMutants(t, "fn f() {\n    x") {
		t.Fatalf("unexpected %s", m.Desc)
	}
}
