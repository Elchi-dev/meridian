package mutate

import (
	"go/ast"
	"go/parser"
	"go/token"
)

// GoMutants finds the mutants of a Go source file from its syntax tree, so
// operator positions are exact even where gofmt drops the spaces around an
// operator to show precedence.
func GoMutants(path string, src []byte) ([]Mutant, error) {
	fset := token.NewFileSet()
	file, err := parser.ParseFile(fset, path, src, parser.SkipObjectResolution)
	if err != nil {
		return nil, err
	}
	c := &collector{path: path, src: src}
	at := func(p token.Pos) int { return fset.Position(p).Offset }
	ast.Inspect(file, func(n ast.Node) bool {
		switch x := n.(type) {
		case *ast.BinaryExpr:
			if x.Op == token.ADD && (joinsStrings(x.X) || joinsStrings(x.Y)) {
				break
			}
			c.operator(at(x.OpPos), x.Op.String())
		case *ast.AssignStmt:
			c.operator(at(x.TokPos), x.Tok.String())
		case *ast.UnaryExpr:
			if x.Op == token.NOT {
				start := at(x.OpPos)
				c.add(start, start+1, "", "! to nothing")
			}
		case *ast.Ident:
			if x.Name == "true" || x.Name == "false" {
				to := map[string]string{"true": "false", "false": "true"}[x.Name]
				start := at(x.Pos())
				c.add(start, start+len(x.Name), to, x.Name+" to "+to)
			}
		case *ast.FuncDecl:
			goBodies(c, x, at)
		}
		return true
	})
	return c.sorted(), nil
}

// joinsStrings reports a string literal, or an expression built on one.
// Mutating the + between strings only gives code that does not compile. In
// valid Go, an operand of + that holds a string literal is itself a string,
// so the operator inside it needs no check.
func joinsStrings(e ast.Expr) bool {
	switch x := e.(type) {
	case *ast.BasicLit:
		return x.Kind == token.STRING
	case *ast.BinaryExpr:
		return joinsStrings(x.X) || joinsStrings(x.Y)
	case *ast.ParenExpr:
		return joinsStrings(x.X)
	}
	return false
}

// goBodies replaces a function's body with the shortest one that compiles
// for its results: nothing for no results, true and false for a bool, nil
// for a lone error.
func goBodies(c *collector, fn *ast.FuncDecl, at func(token.Pos) int) {
	if fn.Body == nil || len(fn.Body.List) == 0 {
		return
	}
	start, end := at(fn.Body.Lbrace), at(fn.Body.Rbrace)+1
	results := fn.Type.Results
	if results == nil || len(results.List) == 0 {
		c.add(start, end, "{}", "body to {}")
		return
	}
	if len(results.List) != 1 || len(results.List[0].Names) > 1 {
		return
	}
	ident, ok := results.List[0].Type.(*ast.Ident)
	if !ok {
		return
	}
	switch ident.Name {
	case "bool":
		c.add(start, end, "{ return true }", "body to { return true }")
		c.add(start, end, "{ return false }", "body to { return false }")
	case "error":
		c.add(start, end, "{ return nil }", "body to { return nil }")
	}
}
