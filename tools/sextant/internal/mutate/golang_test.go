package mutate

import (
	"go/parser"
	"go/token"
	"strings"
	"testing"
)

func goMutants(t *testing.T, src string) []Mutant {
	t.Helper()
	ms, err := GoMutants("x/a.go", []byte(src))
	if err != nil {
		t.Fatalf("GoMutants: %v", err)
	}
	return ms
}

// descs lists the mutants as "line:col desc".
func descs(ms []Mutant) []string {
	out := make([]string, len(ms))
	for i, m := range ms {
		out[i] = itoa(m.Line) + ":" + itoa(m.Col) + " " + m.Desc
	}
	return out
}

func itoa(n int) string {
	if n == 0 {
		return "0"
	}
	var b []byte
	for ; n > 0; n /= 10 {
		b = append([]byte{byte('0' + n%10)}, b...)
	}
	return string(b)
}

func wantDescs(t *testing.T, ms []Mutant, want ...string) {
	t.Helper()
	got := descs(ms)
	if strings.Join(got, "\n") != strings.Join(want, "\n") {
		t.Fatalf("mutants:\n%s\nwant:\n%s", strings.Join(got, "\n"), strings.Join(want, "\n"))
	}
}

func TestGoComparisons(t *testing.T) {
	ms := goMutants(t, "package a\n\nvar x = 1 < 2 || 3 >= 4 && 5 == 6\n")
	wantDescs(t, ms,
		"3:11 < to <=",
		"3:11 < to >=",
		"3:15 || to &&",
		"3:20 >= to <",
		"3:20 >= to >",
		"3:25 && to ||",
		"3:30 == to !=",
	)
}

func TestGoEveryComparisonGetsTwoOrOneMutants(t *testing.T) {
	for op, want := range map[string]string{
		"<":  "< to <=,< to >=",
		"<=": "<= to <,<= to >",
		">":  "> to <=,> to >=",
		">=": ">= to <,>= to >",
		"==": "== to !=",
		"!=": "!= to ==",
	} {
		ms := goMutants(t, "package a\n\nvar x = a "+op+" b\n")
		var got []string
		for _, m := range ms {
			got = append(got, m.Desc)
		}
		if strings.Join(got, ",") != want {
			t.Errorf("%s: %v, want %s", op, got, want)
		}
	}
}

func TestGoArithmeticAndBits(t *testing.T) {
	ms := goMutants(t, "package a\n\nvar x = a*b + c - d/e%f<<1>>2&3|4\n")
	var got []string
	for _, m := range ms {
		got = append(got, m.Desc)
	}
	want := "* to /,+ to -,- to +,/ to *,% to *,<< to >>,>> to <<,& to |,| to &"
	if strings.Join(got, ",") != want {
		t.Fatalf("got %v\nwant %s", got, want)
	}
}

func TestGoTightOperatorsHaveExactPositions(t *testing.T) {
	src := "package a\n\nvar x = a*b + c\n"
	ms := goMutants(t, src)
	if string(ms[0].Apply([]byte(src))) != "package a\n\nvar x = a/b + c\n" {
		t.Fatalf("got %q", ms[0].Apply([]byte(src)))
	}
	if string(ms[1].Apply([]byte(src))) != "package a\n\nvar x = a*b - c\n" {
		t.Fatalf("got %q", ms[1].Apply([]byte(src)))
	}
}

func TestGoStringJoinsAreSkipped(t *testing.T) {
	ms := goMutants(t, "package a\n\nvar s = \"a\" + b + (\"c\" + d)\nvar n = 1 + 2\nvar m = b + \"x\"\nvar p = b + (\"c\" + d)\n")
	wantDescs(t, ms, "4:11 + to -")
}

func TestGoCompoundAssignments(t *testing.T) {
	ms := goMutants(t, "package a\n\nfunc f() int {\n\tx := 1\n\tx += 2\n\tx -= 3\n\tx *= 4\n\tx /= 5\n\tx %= 6\n\tx <<= 1\n\tx >>= 1\n\tx &= 7\n\tx |= 8\n\tx ^= 9\n\tx = 0\n\treturn x\n}\n")
	var got []string
	for _, m := range ms {
		got = append(got, m.Desc)
	}
	want := "+= to -=,-= to +=,*= to /=,/= to *=,%= to *=,<<= to >>=,>>= to <<=,&= to |=,|= to &="
	if strings.Join(got, ",") != want {
		t.Fatalf("got %v\nwant %s", got, want)
	}
}

func TestGoNotAndBooleans(t *testing.T) {
	src := "package a\n\nvar x = !y && true\nvar z = false\n"
	ms := goMutants(t, src)
	wantDescs(t, ms, "3:9 ! to nothing", "3:12 && to ||", "3:15 true to false", "4:9 false to true")
	if got := string(ms[0].Apply([]byte(src))); got != "package a\n\nvar x = y && true\nvar z = false\n" {
		t.Fatalf("removing ! gave %q", got)
	}
	if got := string(ms[3].Apply([]byte(src))); got != "package a\n\nvar x = !y && true\nvar z = true\n" {
		t.Fatalf("swapping false gave %q", got)
	}
}

func TestGoFunctionBodies(t *testing.T) {
	src := `package a

func none() {
	work()
}

func yes() bool {
	return work()
}

func named() (ok bool) {
	return
}

func fails() error {
	return check()
}

func both() (int, error) {
	return 0, nil
}

func pair() (a, b bool) {
	return
}

func text() string {
	return "x"
}

func empty() {}

func unit() () {
	work()
}

func ptr() *bool {
	return nil
}
`
	ms := goMutants(t, src)
	var got []string
	for _, m := range ms {
		got = append(got, itoa(m.Line)+" "+m.Desc)
	}
	want := []string{
		"3 body to {}",
		"7 body to { return false }",
		"7 body to { return true }",
		"11 body to { return false }",
		"11 body to { return true }",
		"15 body to { return nil }",
		"33 body to {}",
	}
	if strings.Join(got, "\n") != strings.Join(want, "\n") {
		t.Fatalf("got\n%s\nwant\n%s", strings.Join(got, "\n"), strings.Join(want, "\n"))
	}
	mutated := ms[1].Apply([]byte(src))
	if !strings.Contains(string(mutated), "func yes() bool { return false }\n") {
		t.Fatalf("body replacement gave:\n%s", mutated)
	}
	for _, m := range ms {
		if _, err := parser.ParseFile(token.NewFileSet(), "a.go", m.Apply([]byte(src)), 0); err != nil {
			t.Errorf("%s does not parse: %v", m.Desc, err)
		}
	}
}

func TestGoMethodsAndClosures(t *testing.T) {
	ms := goMutants(t, "package a\n\nfunc (s *S) Run() {\n\tf := func() { s.n++ }\n\tf()\n}\n")
	wantDescs(t, ms, "3:19 body to {}")
}

func TestGoMutantMetadata(t *testing.T) {
	ms := goMutants(t, "package a\n\nfunc f(a, b int) bool {\n\t\treturn a < b \n}\n")
	m := ms[2]
	if m.Path != "x/a.go" || m.Line != 4 || m.Col != 12 || m.LineText != "return a < b" {
		t.Fatalf("%+v", m)
	}
	if m.String() != "x/a.go:4: < to <=" {
		t.Fatalf("String() = %q", m.String())
	}
	if m.Start != 46 || m.End != 47 || m.To != "<=" {
		t.Fatalf("offsets %d..%d to %q", m.Start, m.End, m.To)
	}
}

func TestGoLastLineWithoutNewline(t *testing.T) {
	ms := goMutants(t, "package a\n\nvar x = a == b")
	if len(ms) != 1 || ms[0].LineText != "var x = a == b" || ms[0].Line != 3 {
		t.Fatalf("%+v", ms)
	}
}

func TestGoSyntaxErrors(t *testing.T) {
	if _, err := GoMutants("x/a.go", []byte("package a\n\nfunc (")); err == nil {
		t.Fatal("broken Go must be an error")
	}
}

func TestApplyKeepsTheRest(t *testing.T) {
	m := Mutant{Start: 2, End: 4, To: "xyz"}
	if got := string(m.Apply([]byte("ab<=cd"))); got != "abxyzcd" {
		t.Fatalf("got %q", got)
	}
	m = Mutant{Start: 0, End: 1, To: ""}
	if got := string(m.Apply([]byte("!x"))); got != "x" {
		t.Fatalf("got %q", got)
	}
}
