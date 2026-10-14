package meri

import (
	"bytes"
	"strconv"
	"strings"
	"unicode/utf8"
)

// MaxDepth is how deep blocks and lists may nest. The root is level 0.
const MaxDepth = 64

type parser struct {
	file string
	src  []byte
	i    int
	line int
	col  int

	// Items and list values collect on these stacks while their block or
	// list is open and are copied out once, at their final size, when it
	// closes. Growing every block's slice on its own was most of the work.
	stack     []Item
	listStack []Value
	names     map[string]string
}

// Parse reads a document. file is only used in positions and error messages.
// The error, when there is one, is always an *Error.
func Parse(file string, src []byte) (*Block, error) {
	p := &parser{file: file, src: src, line: 1, col: 1}
	if bytes.HasPrefix(src, []byte{0xEF, 0xBB, 0xBF}) {
		p.i = 3
	}
	root := &Block{Pos: Pos{File: file, Line: 1, Col: 1}}
	if err := p.items(root, 0); err != nil {
		return nil, err
	}
	return root, nil
}

func (p *parser) pos() Pos { return Pos{File: p.file, Line: p.line, Col: p.col} }

func (p *parser) eof() bool { return p.i >= len(p.src) }

func (p *parser) at(c byte) bool { return p.i < len(p.src) && p.src[p.i] == c }

func (p *parser) skip(n int) {
	p.i += n
	p.col += n
}

func isLetter(c byte) bool   { return c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' }
func isDigit(c byte) bool    { return c >= '0' && c <= '9' }
func isKeyStart(c byte) bool { return isLetter(c) || c == '_' }
func isKeyChar(c byte) bool  { return isKeyStart(c) || isDigit(c) || c == '-' }

// lineEnd reports whether the parser stands at LF or CR LF. A carriage
// return on its own is an error wherever it appears.
func (p *parser) lineEnd() (bool, *Error) {
	if p.eof() {
		return false, nil
	}
	switch p.src[p.i] {
	case '\n':
		return true, nil
	case '\r':
		if p.i+1 < len(p.src) && p.src[p.i+1] == '\n' {
			return true, nil
		}
		return false, errorAt(p.pos(), CodeLoneCarriageReturn, "a carriage return must be followed by a line feed; save the file with LF or CR LF line endings")
	}
	return false, nil
}

func (p *parser) newline() {
	if p.src[p.i] == '\r' {
		p.i++
	}
	p.i++
	p.line++
	p.col = 1
}

// char checks the character at the parser inside a comment, string or text
// and returns its length in bytes. Line endings are the caller's business.
func (p *parser) char() (int, *Error) {
	if err := p.badChar(); err != nil {
		return 0, err
	}
	_, size := utf8.DecodeRune(p.src[p.i:])
	return size, nil
}

// badChar returns the error for a character that is never allowed, or nil
// when the character at the parser is allowed somewhere.
func (p *parser) badChar() *Error {
	c := p.src[p.i]
	switch {
	case c == '\r':
		_, err := p.lineEnd()
		return err
	case c < 0x20 && c != '\t' && c != '\n' || c == 0x7F:
		return errorAt(p.pos(), CodeControlCharacter, "control character U+%04X is not allowed; write it as an escape inside a string, such as \\u{%x}", c, c)
	case c >= 0x80:
		if r, size := utf8.DecodeRune(p.src[p.i:]); r == utf8.RuneError && size == 1 {
			return errorAt(p.pos(), CodeInvalidUTF8, "the file is not valid UTF-8 here; save it as UTF-8")
		}
	}
	return nil
}

// unexpected builds the error for the character at the parser, preferring
// the encoding errors every position shares over the given code.
func (p *parser) unexpected(code, format string, args ...any) *Error {
	if !p.eof() {
		if err := p.badChar(); err != nil {
			return err
		}
	}
	return errorAt(p.pos(), code, format, args...)
}

// describe names the character at the parser for messages.
func (p *parser) describe() string {
	if p.eof() {
		return "the end of the file"
	}
	if ok, _ := p.lineEnd(); ok {
		return "the end of the line"
	}
	r, _ := utf8.DecodeRune(p.src[p.i:])
	if r == '#' {
		return "a comment"
	}
	return strconv.QuoteRune(r)
}

func (p *parser) skipSpaces() {
	for p.i < len(p.src) && (p.src[p.i] == ' ' || p.src[p.i] == '\t') {
		p.skip(1)
	}
}

func (p *parser) skipSpacesAndComment() *Error {
	p.skipSpaces()
	if p.at('#') {
		return p.comment()
	}
	return nil
}

func (p *parser) comment() *Error {
	p.skip(1)
	for p.i < len(p.src) {
		if ok, _ := p.lineEnd(); ok {
			return nil
		}
		n, err := p.char()
		if err != nil {
			return err
		}
		p.i += n
		p.col++
	}
	return nil
}

// skipListSpace skips everything that may sit between the parts of a list,
// line endings included.
func (p *parser) skipListSpace() *Error {
	for {
		if err := p.skipSpacesAndComment(); err != nil {
			return err
		}
		ok, err := p.lineEnd()
		if err != nil {
			return err
		}
		if !ok {
			return nil
		}
		p.newline()
	}
}

func (p *parser) push(b *Block, mark int, it Item) {
	p.stack = append(p.stack, it)
	b.Items = p.stack[mark:len(p.stack):len(p.stack)]
	b.added()
}

func (p *parser) items(b *Block, depth int) *Error {
	mark := len(p.stack)
	defer func() {
		b.Items = append([]Item(nil), p.stack[mark:]...)
		p.stack = p.stack[:mark]
	}()
	for {
		if err := p.skipSpacesAndComment(); err != nil {
			return err
		}
		if p.eof() {
			if depth > 0 {
				return errorAt(b.Pos, CodeUnclosedBlock, "block %q is never closed; add a } for it", b.Name)
			}
			return nil
		}
		c := p.src[p.i]
		switch {
		case c == '\n' || c == '\r':
			if _, err := p.lineEnd(); err != nil {
				return err
			}
			p.newline()
		case c == '}':
			if depth == 0 {
				return errorAt(p.pos(), CodeUnexpectedClose, "this } closes no block; remove it")
			}
			p.skip(1)
			return nil
		case isKeyStart(c):
			if err := p.item(b, mark, depth); err != nil {
				return err
			}
			if err := p.endOfItem(); err != nil {
				return err
			}
		default:
			return p.unexpected(CodeUnexpectedCharacter, "unexpected %s; a line holds a setting such as name = value, or a block such as name { }", p.describe())
		}
	}
}

func (p *parser) endOfItem() *Error {
	if err := p.skipSpacesAndComment(); err != nil {
		return err
	}
	if p.eof() || p.at('}') {
		return nil
	}
	if ok, err := p.lineEnd(); ok || err != nil {
		return err
	}
	return p.unexpected(CodeExpectedNewline, "unexpected %s after the end of the setting; start a new line for the next one", p.describe())
}

// word reads a key, name or bare word. Documents repeat the same few names
// many times, so each distinct one is allocated only once.
func (p *parser) word() string {
	start := p.i
	p.i++
	for p.i < len(p.src) && isKeyChar(p.src[p.i]) {
		p.i++
	}
	p.col += p.i - start
	b := p.src[start:p.i]
	if s, ok := p.names[string(b)]; ok {
		return s
	}
	if p.names == nil {
		p.names = map[string]string{}
	}
	s := string(b)
	p.names[s] = s
	return s
}

func (p *parser) item(b *Block, mark, depth int) *Error {
	at := p.pos()
	name := p.word()
	p.skipSpaces()
	if p.at('=') {
		if err := b.checkEntry(name, at); err != nil {
			return err
		}
		p.skip(1)
		p.skipSpaces()
		v, err := p.value(depth, false)
		if err != nil {
			return err
		}
		p.push(b, mark, Item{Name: name, Pos: at, Value: v})
		return nil
	}
	label, labelled := "", false
	if !p.eof() && (p.src[p.i] == '"' || isKeyStart(p.src[p.i])) {
		var err *Error
		if p.src[p.i] == '"' {
			label, err = p.quoted()
			if err != nil {
				return err
			}
		} else {
			label = p.word()
		}
		labelled = true
		p.skipSpaces()
		if !p.at('{') {
			return p.unexpected(CodeExpectedBrace, "expected { after the label of block %q, found %s", name, p.describe())
		}
	}
	if !p.at('{') {
		return p.unexpected(CodeExpectedEquals, "expected = or { after %q, found %s", name, p.describe())
	}
	if depth+1 > MaxDepth {
		return errorAt(at, CodeTooDeep, "blocks and lists nest more than %d levels deep here", MaxDepth)
	}
	if err := b.checkBlock(name, label, labelled, at); err != nil {
		return err
	}
	p.skip(1)
	child := &Block{Name: name, Label: label, Labelled: labelled, Pos: at}
	if err := p.items(child, depth+1); err != nil {
		return err
	}
	p.push(b, mark, Item{Name: name, Pos: at, Block: child})
	return nil
}

func (b *Block) checkEntry(name string, at Pos) *Error {
	i := b.find(name)
	if i < 0 {
		return nil
	}
	prev := b.Items[i]
	if prev.Block != nil {
		return errorAt(at, CodeNameConflict, "%q is already the name of a block on line %d; a name is either a setting or a block, so rename one of them", name, prev.Pos.Line)
	}
	return errorAt(at, CodeDuplicateKey, "%q is already set on line %d; remove one of them", name, prev.Pos.Line)
}

func (b *Block) checkBlock(name, label string, labelled bool, at Pos) *Error {
	i := b.find(name)
	if i < 0 {
		return nil
	}
	if b.Items[i].Block == nil {
		return errorAt(at, CodeNameConflict, "%q is already set on line %d; a name is either a setting or a block, so rename one of them", name, b.Items[i].Pos.Line)
	}
	if !labelled {
		return nil
	}
	if k := b.labelled(name, label); k >= 0 {
		return errorAt(at, CodeDuplicateLabel, "block %s %s is already defined on line %d; give this one another label", name, quote(label), b.Items[k].Pos.Line)
	}
	return nil
}

func (p *parser) value(depth int, inList bool) (Value, *Error) {
	at := p.pos()
	if p.eof() {
		return Value{}, errorAt(at, CodeExpectedValue, "expected a value, found the end of the file")
	}
	c := p.src[p.i]
	switch {
	case c == '"':
		if bytes.HasPrefix(p.src[p.i:], []byte(`"""`)) {
			s, err := p.text(at)
			return Value{Kind: KindString, Pos: at, Str: s}, err
		}
		s, err := p.quoted()
		return Value{Kind: KindString, Pos: at, Str: s}, err
	case c == '[':
		return p.list(at, depth+1)
	case c == '-' || isDigit(c):
		return p.number(at)
	case isKeyStart(c):
		w := p.word()
		switch w {
		case "true":
			return Value{Kind: KindBool, Pos: at, Bool: true}, nil
		case "false":
			return Value{Kind: KindBool, Pos: at}, nil
		}
		return Value{Kind: KindWord, Pos: at, Str: w}, nil
	case c == '\n' || c == '\r' || c == '#' || c == '}' || inList && c == ',':
		return Value{}, p.unexpected(CodeExpectedValue, "expected a value, found %s", p.describe())
	}
	return Value{}, p.unexpected(CodeUnexpectedCharacter, "%s cannot start a value; values are numbers, true, false, words, \"strings\" or [lists]", p.describe())
}

func (p *parser) quoted() (string, *Error) {
	open := p.pos()
	p.skip(1)
	start := p.i
	var buf []byte
	escaped := false
	for {
		if p.eof() {
			return "", errorAt(open, CodeUnterminatedString, "this string is never closed; add a \" at its end")
		}
		c := p.src[p.i]
		switch {
		case c == '"':
			s := p.src[start:p.i]
			p.skip(1)
			if !escaped {
				return string(s), nil
			}
			return string(append(buf, s...)), nil
		case c == '\\':
			buf = append(buf, p.src[start:p.i]...)
			r, err := p.escape()
			if err != nil {
				return "", err
			}
			buf = utf8.AppendRune(buf, r)
			escaped = true
			start = p.i
		case c >= 0x20 && c < 0x7F:
			// Plain ASCII is most of every string; the general path below
			// accepts it too, only slower.
			p.skip(1)
		default:
			if ok, err := p.lineEnd(); ok || err != nil {
				if err == nil {
					err = errorAt(open, CodeUnterminatedString, "this string is never closed on its line; add a \" at its end, or use \"\"\" for text over several lines")
				}
				return "", err
			}
			n, err := p.char()
			if err != nil {
				return "", err
			}
			p.i += n
			p.col++
		}
	}
}

func simpleEscape(c byte) (rune, bool) {
	switch c {
	case '\\', '"':
		return rune(c), true
	case 'n':
		return '\n', true
	case 'r':
		return '\r', true
	case 't':
		return '\t', true
	case '0':
		return 0, true
	}
	return 0, false
}

func (p *parser) escape() (rune, *Error) {
	at := p.pos()
	p.skip(1)
	if !p.eof() {
		if p.src[p.i] == 'u' {
			return p.unicodeEscape(at)
		}
		if r, ok := simpleEscape(p.src[p.i]); ok {
			p.skip(1)
			return r, nil
		}
	}
	return 0, errorAt(at, CodeInvalidEscape, "a backslash followed by %s is not an escape; the escapes are \\\\, \\\", \\n, \\r, \\t, \\0 and \\u{...}", p.describe())
}

func hexDigit(c byte) (rune, bool) {
	switch {
	case c >= '0' && c <= '9':
		return rune(c - '0'), true
	case c >= 'a' && c <= 'f':
		return rune(c-'a') + 10, true
	case c >= 'A' && c <= 'F':
		return rune(c-'A') + 10, true
	}
	return 0, false
}

func (p *parser) unicodeEscape(at Pos) (rune, *Error) {
	bad := func() (rune, *Error) {
		return 0, errorAt(at, CodeInvalidUnicodeEscape, "\\u needs 1 to 6 hexadecimal digits in braces naming a Unicode character, such as \\u{e9}")
	}
	p.skip(1)
	if !p.at('{') {
		return bad()
	}
	p.skip(1)
	var r rune
	n := 0
	for !p.eof() && p.src[p.i] != '}' {
		d, ok := hexDigit(p.src[p.i])
		if !ok || n == 6 {
			return bad()
		}
		r = r<<4 | d
		n++
		p.skip(1)
	}
	if p.eof() || n == 0 || r > utf8.MaxRune || r >= 0xD800 && r <= 0xDFFF {
		return bad()
	}
	p.skip(1)
	return r, nil
}

func (p *parser) text(at Pos) (string, *Error) {
	p.skip(3)
	p.skipSpaces()
	ok, err := p.lineEnd()
	if err != nil {
		return "", err
	}
	if !ok {
		if p.eof() {
			return "", errorAt(at, CodeUnterminatedText, "this text is never closed; end it with \"\"\" on a line of its own")
		}
		return "", p.unexpected(CodeTextOpening, "nothing may follow the opening \"\"\" on its line; start the text on the next line")
	}
	p.newline()

	var lines []textLine
	for {
		if p.eof() {
			return "", errorAt(at, CodeUnterminatedText, "this text is never closed; end it with \"\"\" on a line of its own")
		}
		start := p.i
		j := p.i
		for j < len(p.src) && (p.src[j] == ' ' || p.src[j] == '\t') {
			j++
		}
		if bytes.HasPrefix(p.src[j:], []byte(`"""`)) {
			indent := p.src[start:j]
			p.skip(j - start + 3)
			return p.joinText(lines, indent)
		}
		for p.i < len(p.src) {
			if ok, _ := p.lineEnd(); ok {
				break
			}
			n, err := p.char()
			if err != nil {
				return "", err
			}
			p.i += n
			p.col++
		}
		lines = append(lines, textLine{start, p.i, p.line})
		if p.eof() {
			return "", errorAt(at, CodeUnterminatedText, "this text is never closed; end it with \"\"\" on a line of its own")
		}
		p.newline()
	}
}

type textLine struct{ start, end, line int }

func onlySpaces(b []byte) bool {
	for _, c := range b {
		if c != ' ' && c != '\t' {
			return false
		}
	}
	return true
}

func (p *parser) joinText(lines []textLine, indent []byte) (string, *Error) {
	var out []byte
	for k, ln := range lines {
		if k > 0 {
			out = append(out, '\n')
		}
		content := p.src[ln.start:ln.end]
		switch {
		case onlySpaces(content):
		case bytes.HasPrefix(content, indent):
			out = append(out, content[len(indent):]...)
		default:
			return "", errorAt(Pos{File: p.file, Line: ln.line, Col: 1}, CodeTextIndent, "this line is indented less than the closing \"\"\"; indent it at least as far, with the same spaces or tabs")
		}
	}
	return string(out), nil
}

func (p *parser) list(at Pos, depth int) (Value, *Error) {
	if depth > MaxDepth {
		return Value{}, errorAt(at, CodeTooDeep, "blocks and lists nest more than %d levels deep here", MaxDepth)
	}
	p.skip(1)
	v := Value{Kind: KindList, Pos: at}
	mark := len(p.listStack)
	defer func() { p.listStack = p.listStack[:mark] }()
	done := func() (Value, *Error) {
		v.List = append([]Value(nil), p.listStack[mark:]...)
		return v, nil
	}
	unterminated := func() (Value, *Error) {
		return Value{}, errorAt(at, CodeUnterminatedList, "this list is never closed; add a ] at its end")
	}
	for {
		if err := p.skipListSpace(); err != nil {
			return Value{}, err
		}
		if p.eof() {
			return unterminated()
		}
		if p.at(']') {
			p.skip(1)
			return done()
		}
		item, err := p.value(depth, true)
		if err != nil {
			return Value{}, err
		}
		p.listStack = append(p.listStack, item)
		if err := p.skipListSpace(); err != nil {
			return Value{}, err
		}
		if p.eof() {
			return unterminated()
		}
		switch p.src[p.i] {
		case ',':
			p.skip(1)
		case ']':
			p.skip(1)
			return done()
		default:
			return Value{}, p.unexpected(CodeExpectedComma, "expected , or ] after the list item, found %s", p.describe())
		}
	}
}

// digitsEnd returns where a run of digits ends that starts with a digit at
// i. A _ belongs to the run only when another digit follows it; anything
// left over is caught by the check after the number.
func digitsEnd(src []byte, i int, digit func(byte) bool) int {
	for i < len(src) {
		if digit(src[i]) {
			i++
			continue
		}
		if src[i] == '_' && i+1 < len(src) && digit(src[i+1]) {
			i += 2
			continue
		}
		break
	}
	return i
}

func isHex(c byte) bool    { _, ok := hexDigit(c); return ok }
func isOctal(c byte) bool  { return c >= '0' && c <= '7' }
func isBinary(c byte) bool { return c == '0' || c == '1' }

func isNumberTail(c byte) bool { return isLetter(c) || isDigit(c) || c == '_' || c == '.' || c == '%' }

func withoutUnderscores(b []byte) string { return strings.ReplaceAll(string(b), "_", "") }

func (p *parser) number(at Pos) (Value, *Error) {
	src := p.src
	start := p.i
	i := p.i
	invalid := func(msg string) (Value, *Error) {
		return Value{}, errorAt(at, CodeInvalidNumber, "%s", msg)
	}
	if src[i] == '-' {
		i++
	}
	if i >= len(src) || !isDigit(src[i]) {
		return invalid("a minus sign must be followed directly by a digit")
	}
	if src[i] == '0' && i+2 < len(src) {
		switch {
		case src[i+1] == 'x' && isHex(src[i+2]):
			return p.radix(at, start, i+2, 16, isHex)
		case src[i+1] == 'o' && isOctal(src[i+2]):
			return p.radix(at, start, i+2, 8, isOctal)
		case src[i+1] == 'b' && isBinary(src[i+2]):
			return p.radix(at, start, i+2, 2, isBinary)
		}
	}
	intStart := i
	i = digitsEnd(src, i, isDigit)
	if src[intStart] == '0' && i-intStart > 1 {
		return invalid("a number cannot start with 0 unless it is 0; write 7 rather than 007, or 0o7 for octal")
	}
	fractional := false
	if i < len(src) && src[i] == '.' {
		if i+1 >= len(src) || !isDigit(src[i+1]) {
			return invalid("a decimal point needs digits on both sides, such as 1.0")
		}
		i = digitsEnd(src, i+1, isDigit)
		fractional = true
	}
	if i < len(src) && (src[i] == 'e' || src[i] == 'E') {
		j := i + 1
		if j < len(src) && (src[j] == '+' || src[j] == '-') {
			j++
		}
		if j < len(src) && isDigit(src[j]) {
			i = digitsEnd(src, j, isDigit)
			fractional = true
		}
	}
	numEnd := i
	if i < len(src) && src[i] == '%' {
		i++
	} else {
		for i < len(src) && isLetter(src[i]) {
			i++
		}
	}
	if i < len(src) && isNumberTail(src[i]) {
		return invalid("this number has extra characters after it; check for a misplaced _ or . or a unit followed by more")
	}
	lit := src[start:numEnd]
	unit := string(src[numEnd:i])
	p.skip(i - start)

	v := Value{Pos: at, Unit: unit}
	if fractional {
		v.Num = floatText(lit)
		f, err := strconv.ParseFloat(v.Num, 64)
		if err != nil {
			return Value{}, errorAt(at, CodeNumberOverflow, "%s is too large for a 64-bit float", v.Num)
		}
		v.Float = f
		v.Kind = KindFloat
	} else {
		n, err := strconv.ParseInt(withoutUnderscores(lit), 10, 64)
		if err != nil {
			return Value{}, errorAt(at, CodeNumberOverflow, "%s is outside the range of a 64-bit integer", withoutUnderscores(lit))
		}
		v.Int = n
		v.Num = strconv.FormatInt(n, 10)
		v.Kind = KindInt
	}
	if unit != "" {
		v.Kind = KindQuantity
	}
	return v, nil
}

// floatText writes a float the way the canonical dump does: without _, with
// a lower-case e and without a + in the exponent.
func floatText(lit []byte) string {
	out := make([]byte, 0, len(lit))
	for k, c := range lit {
		switch {
		case c == '_':
		case c == 'E':
			out = append(out, 'e')
		case c == '+' && k > 0 && (lit[k-1] == 'e' || lit[k-1] == 'E'):
		default:
			out = append(out, c)
		}
	}
	return string(out)
}

func (p *parser) radix(at Pos, start, digits, base int, digit func(byte) bool) (Value, *Error) {
	src := p.src
	end := digitsEnd(src, digits, digit)
	if end < len(src) {
		c := src[end]
		switch {
		case isLetter(c) || c == '%':
			return Value{}, errorAt(at, CodeUnitNotAllowed, "hexadecimal, octal and binary numbers cannot carry a unit; write the value in decimal, such as 16MiB")
		case isNumberTail(c):
			return Value{}, errorAt(at, CodeInvalidNumber, "this number has extra characters after it; check for the misplaced %s", strconv.QuoteRune(rune(c)))
		}
	}
	text := withoutUnderscores(src[digits:end])
	if src[start] == '-' {
		text = "-" + text
	}
	// The digits are known to be valid, so range is the only way to fail.
	n, err := strconv.ParseInt(text, base, 64)
	if err != nil {
		return Value{}, errorAt(at, CodeNumberOverflow, "%s is outside the range of a 64-bit integer", src[start:end])
	}
	p.skip(end - start)
	return Value{Kind: KindInt, Pos: at, Int: n, Num: strconv.FormatInt(n, 10)}, nil
}
