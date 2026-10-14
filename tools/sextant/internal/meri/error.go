package meri

import (
	"fmt"
	"strconv"
)

// Error codes from section 12 of the specification, shared with every other
// implementation, plus the two codes readers use.
const (
	CodeInvalidUTF8          = "invalid-utf8"
	CodeControlCharacter     = "control-character"
	CodeLoneCarriageReturn   = "lone-carriage-return"
	CodeUnexpectedCharacter  = "unexpected-character"
	CodeExpectedEquals       = "expected-equals"
	CodeExpectedBrace        = "expected-brace"
	CodeExpectedValue        = "expected-value"
	CodeExpectedNewline      = "expected-newline"
	CodeExpectedComma        = "expected-comma"
	CodeDuplicateKey         = "duplicate-key"
	CodeNameConflict         = "name-conflict"
	CodeDuplicateLabel       = "duplicate-label"
	CodeUnclosedBlock        = "unclosed-block"
	CodeUnexpectedClose      = "unexpected-close"
	CodeUnterminatedString   = "unterminated-string"
	CodeInvalidEscape        = "invalid-escape"
	CodeInvalidUnicodeEscape = "invalid-unicode-escape"
	CodeTextOpening          = "text-opening"
	CodeTextIndent           = "text-indent"
	CodeUnterminatedText     = "unterminated-text"
	CodeUnterminatedList     = "unterminated-list"
	CodeInvalidNumber        = "invalid-number"
	CodeNumberOverflow       = "number-overflow"
	CodeUnitNotAllowed       = "unit-not-allowed"
	CodeTooDeep              = "too-deep"

	CodeWrongType  = "wrong-type"
	CodeOutOfRange = "out-of-range"
)

// Error is a problem in a document, or in a value a reader asked for.
type Error struct {
	File string
	Line int
	Col  int
	Code string
	Msg  string
}

func (e *Error) Error() string {
	return e.File + ":" + strconv.Itoa(e.Line) + ":" + strconv.Itoa(e.Col) + ": " + e.Msg
}

func errorAt(at Pos, code, format string, args ...any) *Error {
	return &Error{File: at.File, Line: at.Line, Col: at.Col, Code: code, Msg: fmt.Sprintf(format, args...)}
}
