// Package srclex splits Rust, Go, assembly and linker script sources into
// tokens. sextant uses it to count lines of code and to find the operators it
// mutates, so it only needs to get token boundaries right, not grammar.
package srclex

import (
	"bytes"
	"fmt"
	"path/filepath"
	"strings"
	"unicode"
	"unicode/utf8"
)

type Lang uint8

const (
	Rust Lang = iota + 1
	Go
	Asm
	LinkerScript
)

func (l Lang) String() string {
	switch l {
	case Rust:
		return "Rust"
	case Go:
		return "Go"
	case Asm:
		return "assembly"
	case LinkerScript:
		return "linker script"
	}
	return fmt.Sprintf("Lang(%d)", uint8(l))
}

// LangOf picks the language from a file name. The second result is false for
// files sextant does not count, such as Markdown or TOML.
func LangOf(path string) (Lang, bool) {
	switch strings.ToLower(filepath.Ext(path)) {
	case ".rs":
		return Rust, true
	case ".go":
		return Go, true
	case ".s", ".asm":
		return Asm, true
	case ".ld":
		return LinkerScript, true
	}
	return 0, false
}

type Kind uint8

const (
	Ident Kind = iota + 1
	Number
	String
	Char
	Lifetime
	Punct
	Comment
)

func (k Kind) String() string {
	names := [...]string{"", "ident", "number", "string", "char", "lifetime", "punct", "comment"}
	if int(k) < len(names) && k != 0 {
		return names[k]
	}
	return fmt.Sprintf("Kind(%d)", uint8(k))
}

// Token is one lexical token. Start and End are byte offsets into the source;
// Line and EndLine are 1-based and differ for tokens that span lines, such as
// block comments and raw strings.
type Token struct {
	Kind    Kind
	Text    string
	Start   int
	End     int
	Line    int
	EndLine int
}

// Error reports source that cannot be split into tokens, such as an
// unterminated string. Such code would not compile either.
type Error struct {
	Line int
	Col  int
	Msg  string
}

func (e *Error) Error() string { return fmt.Sprintf("%d:%d: %s", e.Line, e.Col, e.Msg) }

var (
	rustPuncts = []string{
		"<<=", ">>=", "...", "..=",
		"::", "->", "=>", "==", "!=", "<=", ">=", "&&", "||",
		"+=", "-=", "*=", "/=", "%=", "^=", "&=", "|=", "<<", ">>", "..",
	}
	goPuncts = []string{
		"<<=", ">>=", "&^=", "...",
		"&&", "||", "<-", "++", "--", "==", "!=", "<=", ">=", ":=",
		"+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "<<", ">>", "&^",
	}
)

// Lex splits src into tokens, comments included.
func Lex(lang Lang, src []byte) ([]Token, error) {
	l := &lexer{src: src, line: 1, lineStart: 0}
	var err error
	switch lang {
	case Rust:
		err = l.run(l.rustToken)
	case Go:
		err = l.run(l.goToken)
	case Asm:
		err = l.run(l.asmToken)
	case LinkerScript:
		err = l.run(l.ldToken)
	default:
		return nil, fmt.Errorf("srclex: unknown language %v", lang)
	}
	if err != nil {
		return nil, err
	}
	return l.toks, nil
}

type lexer struct {
	src       []byte
	pos       int
	line      int
	lineStart int
	toks      []Token

	tokStart     int
	tokLine      int
	tokLineStart int
}

func (l *lexer) run(next func() error) error {
	for l.pos < len(l.src) {
		c := l.src[l.pos]
		if c == '\n' {
			l.advance(1)
			continue
		}
		if c == ' ' || c == '\t' || c == '\r' || c == '\f' || c == '\v' {
			l.pos++
			continue
		}
		l.tokStart, l.tokLine, l.tokLineStart = l.pos, l.line, l.lineStart
		if err := next(); err != nil {
			return err
		}
	}
	return nil
}

func (l *lexer) peek(ahead int) byte {
	if l.pos+ahead < len(l.src) {
		return l.src[l.pos+ahead]
	}
	return 0
}

// advance moves n bytes forward, keeping the line count right.
func (l *lexer) advance(n int) {
	end := min(l.pos+n, len(l.src))
	for ; l.pos < end; l.pos++ {
		if l.src[l.pos] == '\n' {
			l.line++
			l.lineStart = l.pos + 1
		}
	}
}

func (l *lexer) emit(kind Kind) {
	l.toks = append(l.toks, Token{
		Kind:    kind,
		Text:    string(l.src[l.tokStart:l.pos]),
		Start:   l.tokStart,
		End:     l.pos,
		Line:    l.tokLine,
		EndLine: l.line,
	})
}

func (l *lexer) errorf(format string, args ...any) error {
	return &Error{Line: l.tokLine, Col: l.tokStart - l.tokLineStart + 1, Msg: fmt.Sprintf(format, args...)}
}

func (l *lexer) lineComment() {
	for l.pos < len(l.src) && l.src[l.pos] != '\n' {
		l.pos++
	}
	l.emit(Comment)
}

func (l *lexer) blockComment(nested bool) error {
	l.advance(2)
	depth := 1
	for l.pos < len(l.src) {
		switch {
		case nested && l.src[l.pos] == '/' && l.peek(1) == '*':
			depth++
			l.advance(2)
		case l.src[l.pos] == '*' && l.peek(1) == '/':
			depth--
			l.advance(2)
			if depth == 0 {
				l.emit(Comment)
				return nil
			}
		default:
			l.advance(1)
		}
	}
	return l.errorf("block comment is never closed")
}

// quoted reads a string or character literal that ends at the next
// unescaped quote. The opening quote is at l.pos.
func (l *lexer) quoted(quote byte, kind Kind, multiline bool) error {
	l.advance(1)
	for l.pos < len(l.src) {
		c := l.src[l.pos]
		switch {
		case c == '\\':
			l.advance(2)
		case c == quote:
			l.advance(1)
			l.emit(kind)
			return nil
		case c == '\n' && !multiline:
			return l.errorf("%s literal runs into the end of the line", kind)
		default:
			l.advance(1)
		}
	}
	return l.errorf("%s literal is never closed", kind)
}

func (l *lexer) punct(table []string) {
	for _, p := range table {
		if bytes.HasPrefix(l.src[l.pos:], []byte(p)) {
			l.pos += len(p)
			l.emit(Punct)
			return
		}
	}
	_, size := utf8.DecodeRune(l.src[l.pos:])
	l.pos += size
	l.emit(Punct)
}

func (l *lexer) identStartAt(i int) bool {
	if i >= len(l.src) {
		return false
	}
	r, _ := utf8.DecodeRune(l.src[i:])
	return r == '_' || unicode.IsLetter(r)
}

func (l *lexer) ident() {
	for l.pos < len(l.src) {
		r, size := utf8.DecodeRune(l.src[l.pos:])
		if r != '_' && !unicode.IsLetter(r) && !unicode.IsDigit(r) {
			break
		}
		l.pos += size
	}
	l.emit(Ident)
}

func isDigit(c byte) bool { return c >= '0' && c <= '9' }

func isHexOrSuffix(c byte) bool {
	return isDigit(c) || c == '_' || (c|0x20 >= 'a' && c|0x20 <= 'z')
}

// number reads integer and float literals with their suffixes. A dot only
// belongs to the number when a digit follows, so ranges like 0..10 and method
// calls stay separate tokens.
//
// Prefixed literals like 0x1F, 0o17 and 0b101 need no case of their own: the
// leading 0 is read as digits and the rest as a suffix.
func (l *lexer) number() {
	for l.pos < len(l.src) && (isDigit(l.src[l.pos]) || l.src[l.pos] == '_') {
		l.pos++
	}
	if l.peek(0) == '.' && isDigit(l.peek(1)) {
		l.pos++
		for l.pos < len(l.src) && (isDigit(l.src[l.pos]) || l.src[l.pos] == '_') {
			l.pos++
		}
	}
	if c := l.peek(0); c == 'e' || c == 'E' {
		sign := l.peek(1) == '+' || l.peek(1) == '-'
		digitAt := 1
		if sign {
			digitAt = 2
		}
		if isDigit(l.peek(digitAt)) {
			l.pos += digitAt
			for l.pos < len(l.src) && (isDigit(l.src[l.pos]) || l.src[l.pos] == '_') {
				l.pos++
			}
		}
	}
	for l.pos < len(l.src) && isHexOrSuffix(l.src[l.pos]) {
		l.pos++
	}
	l.emit(Number)
}

func (l *lexer) rustToken() error {
	c, next := l.src[l.pos], l.peek(1)
	switch {
	case c == '/' && next == '/':
		l.lineComment()
	case c == '/' && next == '*':
		return l.blockComment(true)
	case c == '"':
		return l.quoted('"', String, true)
	case c == '\'':
		return l.rustQuote()
	case c == 'r' && (next == '"' || (next == '#' && (l.peek(2) == '"' || l.peek(2) == '#'))):
		return l.rawString(1)
	case (c == 'b' || c == 'c') && next == 'r' && (l.peek(2) == '"' || l.peek(2) == '#'):
		return l.rawString(2)
	case (c == 'b' || c == 'c') && next == '"':
		l.pos++
		return l.quoted('"', String, true)
	case c == 'b' && next == '\'':
		l.pos++
		return l.quoted('\'', Char, false)
	case c == 'r' && next == '#' && l.identStartAt(l.pos+2):
		l.pos += 2
		l.ident()
	case isDigit(c):
		l.number()
	case l.identStartAt(l.pos):
		l.ident()
	default:
		l.punct(rustPuncts)
	}
	return nil
}

// rustQuote tells character literals from lifetimes and loop labels. A quote
// followed by an escape, or by one character and a closing quote, is a char.
func (l *lexer) rustQuote() error {
	if l.peek(1) == '\\' {
		return l.quoted('\'', Char, false)
	}
	_, size := utf8.DecodeRune(l.src[min(l.pos+1, len(l.src)):])
	if size > 0 && l.peek(1+size) == '\'' {
		l.pos += 2 + size
		l.emit(Char)
		return nil
	}
	if !l.identStartAt(l.pos + 1) {
		return l.errorf("a quote here starts neither a character nor a lifetime")
	}
	l.pos++
	for l.pos < len(l.src) {
		r, size := utf8.DecodeRune(l.src[l.pos:])
		if r != '_' && !unicode.IsLetter(r) && !unicode.IsDigit(r) {
			break
		}
		l.pos += size
	}
	l.emit(Lifetime)
	return nil
}

// rawString reads r"..." and r#"..."# forms. prefix is the number of letters
// before the hashes (1 for r, 2 for br and cr).
func (l *lexer) rawString(prefix int) error {
	l.pos += prefix
	hashes := 0
	for l.peek(0) == '#' {
		hashes++
		l.pos++
	}
	if l.peek(0) != '"' {
		return l.errorf("raw string needs a quote after its hashes")
	}
	l.advance(1)
	closing := "\"" + strings.Repeat("#", hashes)
	for l.pos < len(l.src) {
		if bytes.HasPrefix(l.src[l.pos:], []byte(closing)) {
			l.advance(len(closing))
			l.emit(String)
			return nil
		}
		l.advance(1)
	}
	return l.errorf("raw string is never closed")
}

func (l *lexer) goToken() error {
	c, next := l.src[l.pos], l.peek(1)
	switch {
	case c == '/' && next == '/':
		l.lineComment()
	case c == '/' && next == '*':
		return l.blockComment(false)
	case c == '"':
		return l.quoted('"', String, false)
	case c == '`':
		l.advance(1)
		for l.pos < len(l.src) && l.src[l.pos] != '`' {
			l.advance(1)
		}
		if l.pos >= len(l.src) {
			return l.errorf("raw string is never closed")
		}
		l.advance(1)
		l.emit(String)
	case c == '\'':
		return l.quoted('\'', Char, false)
	case isDigit(c) || (c == '.' && isDigit(next)):
		l.number()
	case l.identStartAt(l.pos):
		l.ident()
	default:
		l.punct(goPuncts)
	}
	return nil
}

func isAsmWord(c byte) bool {
	return isHexOrSuffix(c) || c == '.' || c == '$' || c == '%' || c == '@'
}

func (l *lexer) asmToken() error {
	c, next := l.src[l.pos], l.peek(1)
	switch {
	case c == '#' || (c == '/' && next == '/'):
		l.lineComment()
	case c == '/' && next == '*':
		return l.blockComment(false)
	case c == '"':
		return l.quoted('"', String, false)
	case isAsmWord(c):
		for l.pos < len(l.src) && isAsmWord(l.src[l.pos]) {
			l.pos++
		}
		l.emit(Ident)
	default:
		l.punct(nil)
	}
	return nil
}

func (l *lexer) ldToken() error {
	c, next := l.src[l.pos], l.peek(1)
	switch {
	case c == '/' && next == '*':
		return l.blockComment(false)
	case c == '"':
		return l.quoted('"', String, false)
	case isAsmWord(c):
		for l.pos < len(l.src) && isAsmWord(l.src[l.pos]) {
			l.pos++
		}
		l.emit(Ident)
	default:
		l.punct(nil)
	}
	return nil
}

type LineKind uint8

const (
	Blank LineKind = iota
	CommentOnly
	Code
)

// LineCount returns the number of lines in src. A final line without a
// newline still counts; an empty source has no lines.
func LineCount(src []byte) int {
	n := bytes.Count(src, []byte{'\n'})
	if len(src) > 0 && src[len(src)-1] != '\n' {
		n++
	}
	return n
}

// Classify marks each of the lines lines as blank, comment only or code. A
// token that spans lines, like a multi-line string, makes every line it
// touches a code line. The result is indexed from 0 for line 1.
func Classify(lines int, toks []Token) []LineKind {
	kinds := make([]LineKind, lines)
	for _, t := range toks {
		mark := Code
		if t.Kind == Comment {
			mark = CommentOnly
		}
		for ln := t.Line; ln <= t.EndLine && ln <= lines; ln++ {
			if kinds[ln-1] < mark {
				kinds[ln-1] = mark
			}
		}
	}
	return kinds
}
