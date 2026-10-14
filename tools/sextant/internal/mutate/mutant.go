// Package mutate changes code on purpose, one small change at a time, and
// runs the tests after each. A change that no test notices shows code the
// tests do not really check (ADR 0009).
package mutate

import (
	"bytes"
	"cmp"
	"fmt"
	"slices"
	"strings"
)

// Mutant is one change to one source file.
type Mutant struct {
	Path     string
	Line     int
	Col      int
	Start    int
	End      int
	To       string
	Desc     string
	LineText string
}

// Apply returns src with the mutant's change made.
func (m Mutant) Apply(src []byte) []byte {
	return slices.Concat(src[:m.Start], []byte(m.To), src[m.End:])
}

func (m Mutant) String() string {
	return fmt.Sprintf("%s:%d: %s", m.Path, m.Line, m.Desc)
}

// swaps lists, for every operator that gets mutated, what it becomes.
// Comparisons get two mutants each: the negation and the shifted boundary.
var swaps = map[string][]string{
	"==": {"!="}, "!=": {"=="},
	"<": {"<=", ">="}, "<=": {"<", ">"}, ">": {">=", "<="}, ">=": {">", "<"},
	"&&": {"||"}, "||": {"&&"},
	"+": {"-"}, "-": {"+"}, "*": {"/"}, "/": {"*"}, "%": {"*"},
	"<<": {">>"}, ">>": {"<<"}, "&": {"|"}, "|": {"&"},
	"+=": {"-="}, "-=": {"+="}, "*=": {"/="}, "/=": {"*="}, "%=": {"*="},
	"<<=": {">>="}, ">>=": {"<<="}, "&=": {"|="}, "|=": {"&="},
}

type collector struct {
	path string
	src  []byte
	out  []Mutant
}

// add records a mutant that replaces src[start:end] with to.
func (c *collector) add(start, end int, to, desc string) {
	lineStart := bytes.LastIndexByte(c.src[:start], '\n') + 1
	line, _, _ := bytes.Cut(c.src[lineStart:], []byte{'\n'})
	c.out = append(c.out, Mutant{
		Path:     c.path,
		Line:     1 + bytes.Count(c.src[:start], []byte{'\n'}),
		Col:      start - lineStart + 1,
		Start:    start,
		End:      end,
		To:       to,
		Desc:     desc,
		LineText: string(bytes.TrimSpace(line)),
	})
}

// operator adds one mutant per replacement of the operator at start.
func (c *collector) operator(start int, op string) {
	for _, to := range swaps[op] {
		c.add(start, start+len(op), to, op+" to "+to)
	}
}

func (c *collector) sorted() []Mutant {
	slices.SortStableFunc(c.out, func(a, b Mutant) int {
		return cmp.Or(cmp.Compare(a.Start, b.Start), strings.Compare(a.Desc, b.Desc))
	})
	return c.out
}
