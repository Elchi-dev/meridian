package mutate

import (
	"bytes"
	"strings"

	"github.com/Elchi-dev/meridian/tools/sextant/internal/srclex"
)

// RustMutants finds the mutants of a Rust source file. Lines for which
// isTest reports true are test code and are left alone.
//
// Rust has no parser in the standard library, so this works on tokens and
// leans on rustfmt: it always puts spaces around binary operators and never
// around generics, references, closure bars or macro bangs.
func RustMutants(path string, src []byte, isTest func(line int) bool) ([]Mutant, error) {
	all, err := srclex.Lex(srclex.Rust, src)
	if err != nil {
		return nil, err
	}
	toks := make([]srclex.Token, 0, len(all))
	for _, t := range all {
		if t.Kind != srclex.Comment {
			toks = append(toks, t)
		}
	}
	inAttr := attributes(toks)
	c := &collector{path: path, src: src}
	for i, t := range toks {
		if inAttr[i] || isTest(t.Line) {
			continue
		}
		switch {
		case t.Kind == srclex.Punct && t.Text == "!":
			if unaryNot(src, toks, i) {
				c.add(t.Start, t.End, "", "! to nothing")
			}
		case swaps[t.Text] != nil:
			if spacedBinary(src, toks, i) && !(t.Text == "|" && armPattern(src, t)) {
				c.operator(t.Start, t.Text)
			}
		case t.Kind == srclex.Ident && (t.Text == "true" || t.Text == "false"):
			to := map[string]string{"true": "false", "false": "true"}[t.Text]
			c.add(t.Start, t.End, to, t.Text+" to "+to)
		case t.Kind == srclex.Ident && t.Text == "fn":
			rustBody(c, toks, i)
		}
	}
	return c.sorted(), nil
}

// attributes marks every token of #[...] and #![...].
func attributes(toks []srclex.Token) []bool {
	marked := make([]bool, len(toks))
	for i := 0; i < len(toks); i++ {
		if toks[i].Text != "#" {
			continue
		}
		open := i + 1
		if open < len(toks) && toks[open].Text == "!" {
			open++
		}
		if open >= len(toks) || toks[open].Text != "[" {
			continue
		}
		end := closer(toks, open)
		for k := i; k <= end; k++ {
			marked[k] = true
		}
		i = end
	}
	return marked
}

// closer returns the index of the bracket that closes the one at open, or
// the last token when the file ends first.
func closer(toks []srclex.Token, open int) int {
	depth := 0
	for i := open; i < len(toks); i++ {
		switch toks[i].Text {
		case "(", "[", "{":
			depth++
		case ")", "]", "}":
			depth--
			if depth == 0 {
				return i
			}
		}
	}
	return len(toks) - 1
}

// Keywords after which an expression starts, so the next operator is unary
// or part of a type, never binary.
var startsExpression = map[string]bool{
	"as": true, "break": true, "const": true, "continue": true, "dyn": true, "else": true,
	"enum": true, "extern": true, "fn": true, "for": true, "if": true, "impl": true,
	"in": true, "let": true, "loop": true, "match": true, "mod": true, "move": true,
	"mut": true, "pub": true, "ref": true, "return": true, "static": true, "struct": true,
	"trait": true, "type": true, "unsafe": true, "use": true, "where": true, "while": true,
}

// endsOperand reports whether an expression can end with the token, which
// is what a binary operator needs on its left.
func endsOperand(t srclex.Token) bool {
	switch t.Kind {
	case srclex.Ident:
		return !startsExpression[t.Text]
	case srclex.Number, srclex.String, srclex.Char:
		return true
	case srclex.Punct:
		return t.Text == ")" || t.Text == "]" || t.Text == "?"
	}
	return false
}

func spacedBinary(src []byte, toks []srclex.Token, i int) bool {
	t := toks[i]
	return i > 0 && t.End < len(src) &&
		src[t.Start-1] == ' ' && src[t.End] == ' ' && endsOperand(toks[i-1])
}

// armPattern reports a | that joins the patterns of a match arm.
func armPattern(src []byte, t srclex.Token) bool {
	rest, _, _ := bytes.Cut(src[t.End:], []byte{'\n'})
	return bytes.Contains(rest, []byte("=>"))
}

// unaryNot reports a ! that negates the expression right after it, as
// opposed to a macro call, an inner attribute or the never type.
func unaryNot(src []byte, toks []srclex.Token, i int) bool {
	t := toks[i]
	if t.End >= len(src) || src[t.End] == ' ' || src[t.End] == '\n' {
		return false
	}
	return i == 0 || !endsOperand(toks[i-1])
}

// rustBody replaces a function's body with the shortest one that compiles
// for its return type: {} for (), true and false for bool, Ok(()) for a
// Result with a unit value, an alias such as io::Result<()> included.
// Functions without a body are left alone.
func rustBody(c *collector, toks []srclex.Token, fn int) {
	name := fn + 1
	if name >= len(toks) || toks[name].Kind != srclex.Ident {
		return
	}
	params := -1
	angle := 0
	for k := name; k < len(toks) && params == -1; k++ {
		switch toks[k].Text {
		case "<":
			angle++
		case ">":
			angle--
		case ">>":
			angle -= 2
		case "(":
			if angle <= 0 {
				params = k
			}
		case ";", "{":
			return
		}
	}
	if params == -1 {
		return
	}
	afterParams := closer(toks, params) + 1
	body, retEnd := -1, -1
	depth := 0
	for k := afterParams; k < len(toks) && body == -1; k++ {
		switch toks[k].Text {
		case "(", "[":
			depth++
		case ")", "]":
			depth--
		case "where":
			if depth == 0 {
				retEnd = k
			}
		case ";":
			if depth == 0 {
				return
			}
		case "{":
			if depth == 0 {
				body = k
			}
		}
	}
	if body == -1 {
		return
	}
	if retEnd == -1 {
		retEnd = body
	}
	end := closer(toks, body)
	if end == body+1 {
		return
	}
	var ret []string
	if toks[afterParams].Text == "->" {
		for _, t := range toks[afterParams+1 : retEnd] {
			ret = append(ret, t.Text)
		}
	}
	start, stop := toks[body].Start, toks[end].End
	switch joined := strings.Join(ret, " "); {
	case joined == "" || joined == "( )":
		c.add(start, stop, "{}", "body to {}")
	case joined == "bool":
		c.add(start, stop, "{ true }", "body to { true }")
		c.add(start, stop, "{ false }", "body to { false }")
	case strings.HasPrefix(joined, "Result < ( )"):
		c.add(start, stop, "{ Ok(()) }", "body to { Ok(()) }")
	}
}
