// Package stats counts lines of code and lines of tests per component, so the
// rule of at least 1.5 test lines for every line of code can be checked by a
// program instead of by memory.
package stats

import (
	"bytes"
	"fmt"
	"io/fs"
	"math"
	"path"
	"slices"
	"strings"

	"github.com/Elchi-dev/meridian/tools/sextant/internal/srclex"
)

// File is the count for one source file. Code and Test hold lines with code
// on them; comment-only and blank lines are counted separately and never
// count toward the ratio.
type File struct {
	Path     string
	Lang     srclex.Lang
	Code     int
	Test     int
	Comments int
	Blank    int
	Unsafe   int
}

// Component is a Rust crate or a Go module: the unit the ratio applies to.
type Component struct {
	Path   string
	Kind   string
	Files  []File
	Code   int
	Test   int
	Unsafe int
}

// Ratio is test lines per line of code. A component without code has
// nothing that needs testing and reports +Inf.
func (c *Component) Ratio() float64 {
	if c.Code == 0 {
		return math.Inf(1)
	}
	return float64(c.Test) / float64(c.Code)
}

// Missing is how many more test lines the component needs to reach min.
func (c *Component) Missing(min float64) int {
	need := int(math.Ceil(min*float64(c.Code) - 1e-9))
	return max(need-c.Test, 0)
}

type Report struct {
	Components []*Component
}

// Failing returns the components whose ratio is below min, in path order.
func (r *Report) Failing(min float64) []*Component {
	var out []*Component
	for _, c := range r.Components {
		if c.Missing(min) > 0 {
			out = append(out, c)
		}
	}
	return out
}

var skipDirs = map[string]bool{"target": true, "out": true, "testdata": true, "node_modules": true}

// Collect walks fsys from its root and counts every Rust, Go, assembly and
// linker script file that belongs to a component. Files outside any
// component, such as conformance data, are not counted.
func Collect(fsys fs.FS) (*Report, error) {
	sc, err := scan(fsys)
	if err != nil {
		return nil, err
	}
	for _, src := range sc.sources {
		f := src.parsed.count(src.Path, src.AllTest)
		c := sc.comps[src.Component]
		c.Files = append(c.Files, f)
		c.Code += f.Code
		c.Test += f.Test
		c.Unsafe += f.Unsafe
	}
	report := &Report{}
	for _, c := range sc.comps {
		slices.SortFunc(c.Files, func(a, b File) int { return strings.Compare(a.Path, b.Path) })
		report.Components = append(report.Components, c)
	}
	slices.SortFunc(report.Components, func(a, b *Component) int { return strings.Compare(a.Path, b.Path) })
	return report, nil
}

// Source is one counted file with what tools beyond counting need: its
// component, and which of its lines are test code.
type Source struct {
	Path      string
	Lang      srclex.Lang
	Component string
	Kind      string
	AllTest   bool
	TestLines []bool

	parsed *parsedFile
}

// IsTest reports whether line, counted from 1, is test code.
func (s *Source) IsTest(line int) bool {
	return s.AllTest || (line >= 1 && line <= len(s.TestLines) && s.TestLines[line-1])
}

// Sources returns every file Collect counts, in path order, with the same
// rules for what is test code.
func Sources(fsys fs.FS) ([]Source, error) {
	sc, err := scan(fsys)
	if err != nil {
		return nil, err
	}
	return sc.sources, nil
}

type scanned struct {
	comps   map[string]*Component
	sources []Source
}

func scan(fsys fs.FS) (*scanned, error) {
	comps := map[string]*Component{}
	var paths []string
	err := fs.WalkDir(fsys, ".", func(p string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		name := d.Name()
		if d.IsDir() {
			if p != "." && (strings.HasPrefix(name, ".") || skipDirs[name]) {
				return fs.SkipDir
			}
			return nil
		}
		switch name {
		case "go.mod":
			comps[path.Dir(p)] = &Component{Path: path.Dir(p), Kind: "go"}
		case "Cargo.toml":
			manifest, err := fs.ReadFile(fsys, p)
			if err != nil {
				return err
			}
			if hasPackageTable(manifest) {
				comps[path.Dir(p)] = &Component{Path: path.Dir(p), Kind: "rust"}
			}
		}
		if _, ok := srclex.LangOf(name); ok {
			paths = append(paths, p)
		}
		return nil
	})
	if err != nil {
		return nil, err
	}

	parsed := map[string]*parsedFile{}
	testFiles := map[string]bool{}
	var testDirs []string
	for _, p := range paths {
		if owner(comps, p) == nil {
			continue
		}
		pf, err := parse(fsys, p)
		if err != nil {
			return nil, err
		}
		parsed[p] = pf
		for _, mod := range pf.testModules {
			dir := moduleDir(p)
			testFiles[path.Join(dir, mod+".rs")] = true
			testDirs = append(testDirs, path.Join(dir, mod)+"/")
		}
	}

	sc := &scanned{comps: comps}
	for _, p := range paths {
		pf := parsed[p]
		if pf == nil {
			continue
		}
		c := owner(comps, p)
		sc.sources = append(sc.sources, Source{
			Path:      p,
			Lang:      pf.lang,
			Component: c.Path,
			Kind:      c.Kind,
			AllTest:   pf.wholeFileIsTest || testFiles[p] || hasAnyPrefix(p, testDirs) || isTestPath(c.Path, p),
			TestLines: pf.testLines,
			parsed:    pf,
		})
	}
	return sc, nil
}

func hasPackageTable(manifest []byte) bool {
	for _, line := range bytes.Split(manifest, []byte("\n")) {
		if string(bytes.TrimSpace(line)) == "[package]" {
			return true
		}
	}
	return false
}

// owner returns the innermost component that contains p.
func owner(comps map[string]*Component, p string) *Component {
	for dir := path.Dir(p); ; dir = path.Dir(dir) {
		if c, ok := comps[dir]; ok {
			return c
		}
		if dir == "." || dir == "/" {
			return nil
		}
	}
}

func hasAnyPrefix(s string, prefixes []string) bool {
	for _, p := range prefixes {
		if strings.HasPrefix(s, p) {
			return true
		}
	}
	return false
}

// isTestPath reports files that are tests by where they live: Go test files,
// and Rust integration tests and benchmarks in the crate's tests/ and benches/.
func isTestPath(component, p string) bool {
	if strings.HasSuffix(p, "_test.go") {
		return true
	}
	rel := strings.TrimPrefix(p, component+"/")
	if component == "." {
		rel = p
	}
	return strings.HasPrefix(rel, "tests/") || strings.HasPrefix(rel, "benches/")
}

// moduleDir is where Rust looks for the child modules of the file at p:
// next to lib.rs, main.rs and mod.rs, and in a folder named after any other file.
func moduleDir(p string) string {
	base := path.Base(p)
	if base == "lib.rs" || base == "main.rs" || base == "mod.rs" {
		return path.Dir(p)
	}
	return strings.TrimSuffix(p, ".rs")
}

type parsedFile struct {
	lang            srclex.Lang
	kinds           []srclex.LineKind
	testLines       []bool
	unsafeLines     []int
	testModules     []string
	wholeFileIsTest bool
}

func parse(fsys fs.FS, p string) (*parsedFile, error) {
	src, err := fs.ReadFile(fsys, p)
	if err != nil {
		return nil, err
	}
	lang, _ := srclex.LangOf(p)
	toks, err := srclex.Lex(lang, src)
	if err != nil {
		return nil, fmt.Errorf("%s:%w", p, err)
	}
	lines := srclex.LineCount(src)
	pf := &parsedFile{
		lang:      lang,
		kinds:     srclex.Classify(lines, toks),
		testLines: make([]bool, lines),
	}
	if lang != srclex.Rust {
		return pf, nil
	}
	regions := rustTestRegions(toks)
	pf.testModules = regions.modules
	pf.wholeFileIsTest = regions.wholeFile
	for _, r := range regions.lines {
		for ln := r[0]; ln <= r[1] && ln <= lines; ln++ {
			pf.testLines[ln-1] = true
		}
	}
	for i, t := range toks {
		if t.Text == "unsafe" && !isFnPointerType(toks, i) {
			pf.unsafeLines = append(pf.unsafeLines, t.Line)
		}
	}
	return pf, nil
}

// isFnPointerType says whether the unsafe at toks[i] starts the type of a
// function pointer, such as unsafe extern "efiapi" fn(...). Such a type
// does nothing unsafe itself; calling through it needs an unsafe block,
// which counts where it is.
func isFnPointerType(toks []srclex.Token, i int) bool {
	next := func(j int) int {
		for j++; j < len(toks) && toks[j].Kind == srclex.Comment; j++ {
		}
		return j
	}
	j := next(i)
	if j < len(toks) && toks[j].Text == "extern" {
		j = next(j)
		if j < len(toks) && toks[j].Kind == srclex.String {
			j = next(j)
		}
	}
	if j >= len(toks) || toks[j].Text != "fn" {
		return false
	}
	j = next(j)
	return j < len(toks) && toks[j].Text == "("
}

func (pf *parsedFile) count(p string, wholeFileIsTest bool) File {
	f := File{Path: p, Lang: pf.lang}
	for i, k := range pf.kinds {
		switch {
		case k == srclex.Blank:
			f.Blank++
		case k == srclex.CommentOnly:
			f.Comments++
		case wholeFileIsTest || pf.testLines[i]:
			f.Test++
		default:
			f.Code++
		}
	}
	for _, ln := range pf.unsafeLines {
		if !wholeFileIsTest && !pf.testLines[ln-1] {
			f.Unsafe++
		}
	}
	return f
}

type testRegions struct {
	lines     [][2]int
	modules   []string
	wholeFile bool
}

// rustTestRegions finds the items compiled only for tests: anything under
// #[cfg(test)] (or a cfg naming the qemu-test feature), and whole files marked
// with #![cfg(test)]. Test modules are also returned by name, because their
// child modules live in files of their own that count as tests too.
func rustTestRegions(toks []srclex.Token) testRegions {
	var out testRegions
	for i := 0; i < len(toks); i++ {
		if toks[i].Text != "#" {
			continue
		}
		inner := is(toks, i+1, "!")
		open := i + 1
		if inner {
			open++
		}
		if !is(toks, open, "[") {
			continue
		}
		end := matching(toks, open)
		if end == -1 {
			return out
		}
		if !isTestCfg(toks[open+1 : end]) {
			continue
		}
		if inner {
			out.wholeFile = true
			return out
		}
		// The item, its attributes included, runs from the # to last.
		last := itemEnd(toks, i)
		out.lines = append(out.lines, [2]int{toks[i].Line, toks[last].EndLine})
		if name := moduleName(toks[i : last+1]); name != "" {
			out.modules = append(out.modules, name)
		}
		i = last
	}
	return out
}

// is compares token text only. That is enough: punctuation, keywords and
// identifiers cannot be confused with strings or comments, which keep their
// quotes and slashes in their text.
func is(toks []srclex.Token, i int, text string) bool {
	return i < len(toks) && toks[i].Text == text
}

// matching returns the index of the bracket that closes the one at open, or
// -1 when the file ends first.
func matching(toks []srclex.Token, open int) int {
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
	return -1
}

func isTestCfg(attr []srclex.Token) bool {
	if len(attr) == 0 || attr[0].Text != "cfg" {
		return false
	}
	test := false
	for _, t := range attr[1:] {
		switch t.Text {
		case "not":
			return false
		case "test", `"qemu-test"`:
			test = true
		}
	}
	return test
}

// itemEnd finds the last token of the item starting at from: the semicolon
// that ends it, or the brace that closes its body. Brackets and parentheses
// on the way, such as further attributes or a parameter list, are skipped.
func itemEnd(toks []srclex.Token, from int) int {
	for i := from; i < len(toks); i++ {
		switch toks[i].Text {
		case ";":
			return i
		case "{", "(", "[":
			end := matching(toks, i)
			if end == -1 {
				return len(toks) - 1
			}
			if toks[i].Text == "{" {
				return end
			}
			i = end
		}
	}
	return len(toks) - 1
}

// moduleName returns the name of a `mod` item, inline or out of line, and ""
// for every other item. Only the item's header is searched.
func moduleName(item []srclex.Token) string {
	for i := 0; i+1 < len(item); i++ {
		switch item[i].Text {
		case "mod":
			return item[i+1].Text
		case "{":
			return ""
		}
	}
	return ""
}
