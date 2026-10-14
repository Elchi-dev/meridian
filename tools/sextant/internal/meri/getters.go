package meri

import (
	"math/big"
	"time"
)

// Each getter returns def when the key is missing and an *Error naming the
// value's position when it is present with the wrong type, so a typo in a
// file never turns into a silent default.

func (b *Block) entry(key string) (Item, bool) {
	i := b.find(key)
	if i < 0 {
		return Item{}, false
	}
	return b.Items[i], true
}

func describe(v Value) string {
	switch v.Kind {
	case KindBool:
		if v.Bool {
			return "true"
		}
		return "false"
	case KindInt, KindFloat:
		return "the number " + v.Num
	case KindQuantity:
		return "the quantity " + v.Num + v.Unit
	case KindString:
		return "the string " + quote(v.Str)
	case KindWord:
		return "the word " + v.Str
	case KindList:
		return "a list"
	}
	return "a value of kind " + v.Kind.String()
}

func wrongType(key string, it Item, want string) *Error {
	found := "a block"
	if it.Block == nil {
		found = describe(it.Value)
	}
	return errorAt(it.Pos, CodeWrongType, "%s must be %s, found %s", key, want, found)
}

// value returns the entry's value, or a wrong-type error pointing at the
// block when the name belongs to blocks.
func (b *Block) value(key, want string) (Value, bool, *Error) {
	it, ok := b.entry(key)
	if !ok {
		return Value{}, false, nil
	}
	if it.Block != nil {
		return Value{}, true, wrongType(key, it, want)
	}
	return it.Value, true, nil
}

func asText(v Value) (string, bool) {
	if v.Kind == KindString || v.Kind == KindWord {
		return v.Str, true
	}
	return "", false
}

const (
	wantText     = "text such as \"Inter\" or a word such as kvm"
	wantInt      = "a whole number such as 4"
	wantFloat    = "a number such as 1.5"
	wantBool     = "true or false"
	wantSize     = "a size such as 512MiB"
	wantDuration = "a duration such as 30s"
	wantTexts    = "a list of text such as [\"a\", \"b\"]"
)

func mismatch(key string, v Value, want string) *Error {
	return wrongType(key, Item{Pos: v.Pos, Value: v}, want)
}

// String accepts strings and words.
func (b *Block) String(key, def string) (string, error) {
	v, ok, err := b.value(key, wantText)
	if err != nil || !ok {
		return def, nilIfEmpty(err)
	}
	if s, ok := asText(v); ok {
		return s, nil
	}
	return def, mismatch(key, v, wantText)
}

func (b *Block) Int(key string, def int64) (int64, error) {
	v, ok, err := b.value(key, wantInt)
	if err != nil || !ok {
		return def, nilIfEmpty(err)
	}
	if v.Kind != KindInt {
		return def, mismatch(key, v, wantInt)
	}
	return v.Int, nil
}

// Float accepts integers too.
func (b *Block) Float(key string, def float64) (float64, error) {
	v, ok, err := b.value(key, wantFloat)
	if err != nil || !ok {
		return def, nilIfEmpty(err)
	}
	switch v.Kind {
	case KindFloat:
		return v.Float, nil
	case KindInt:
		return float64(v.Int), nil
	}
	return def, mismatch(key, v, wantFloat)
}

func (b *Block) Bool(key string, def bool) (bool, error) {
	v, ok, err := b.value(key, wantBool)
	if err != nil || !ok {
		return def, nilIfEmpty(err)
	}
	if v.Kind != KindBool {
		return def, mismatch(key, v, wantBool)
	}
	return v.Bool, nil
}

// Strings returns a list whose items are all strings or words.
func (b *Block) Strings(key string, def []string) ([]string, error) {
	v, ok, err := b.value(key, wantTexts)
	if err != nil || !ok {
		return def, nilIfEmpty(err)
	}
	if v.Kind != KindList {
		return def, mismatch(key, v, wantTexts)
	}
	out := make([]string, 0, len(v.List))
	for i, e := range v.List {
		s, ok := asText(e)
		if !ok {
			return def, errorAt(e.Pos, CodeWrongType, "%s must be %s, but item %d is %s", key, wantTexts, i+1, describe(e))
		}
		out = append(out, s)
	}
	return out, nil
}

var sizeUnits = map[string]int64{
	"B":   1,
	"KB":  1000,
	"MB":  1000 * 1000,
	"GB":  1000 * 1000 * 1000,
	"TB":  1000 * 1000 * 1000 * 1000,
	"KiB": 1 << 10,
	"MiB": 1 << 20,
	"GiB": 1 << 30,
	"TiB": 1 << 40,
}

var durationUnits = map[string]int64{
	"ns":  int64(time.Nanosecond),
	"us":  int64(time.Microsecond),
	"ms":  int64(time.Millisecond),
	"s":   int64(time.Second),
	"min": int64(time.Minute),
	"h":   int64(time.Hour),
	"d":   int64(24 * time.Hour),
}

// scaled multiplies a quantity's number by a unit's factor exactly and
// reports whether the result is a whole number and whether it fits 64 bits.
func scaled(num string, factor int64) (n int64, whole, fits bool) {
	r, ok := new(big.Rat).SetString(num)
	if !ok {
		return 0, false, false
	}
	r.Mul(r, new(big.Rat).SetInt64(factor))
	if !r.IsInt() {
		return 0, false, false
	}
	if !r.Num().IsInt64() {
		return 0, true, false
	}
	return r.Num().Int64(), true, true
}

// Size returns a size in bytes. The result is exact: 1.5KiB is 1536.
func (b *Block) Size(key string, def int64) (int64, error) {
	v, ok, err := b.value(key, wantSize)
	if err != nil || !ok {
		return def, nilIfEmpty(err)
	}
	factor, known := sizeUnits[v.Unit]
	if v.Kind != KindQuantity || !known {
		return def, mismatch(key, v, wantSize)
	}
	n, whole, fits := scaled(v.Num, factor)
	switch {
	case !whole:
		return def, errorAt(v.Pos, CodeOutOfRange, "%s must be a whole number of bytes, and %s%s is not", key, v.Num, v.Unit)
	case !fits:
		return def, errorAt(v.Pos, CodeOutOfRange, "%s is too large; sizes must fit in 64 bits", key)
	case n < 0:
		return def, errorAt(v.Pos, CodeOutOfRange, "%s cannot be negative", key)
	}
	return n, nil
}

// Duration returns a duration. Negative durations are allowed.
func (b *Block) Duration(key string, def time.Duration) (time.Duration, error) {
	v, ok, err := b.value(key, wantDuration)
	if err != nil || !ok {
		return def, nilIfEmpty(err)
	}
	factor, known := durationUnits[v.Unit]
	if v.Kind != KindQuantity || !known {
		return def, mismatch(key, v, wantDuration)
	}
	n, whole, fits := scaled(v.Num, factor)
	switch {
	case !whole:
		return def, errorAt(v.Pos, CodeOutOfRange, "%s must be a whole number of nanoseconds, and %s%s is not", key, v.Num, v.Unit)
	case !fits:
		return def, errorAt(v.Pos, CodeOutOfRange, "%s is too long; durations must fit in 64 bits of nanoseconds", key)
	}
	return time.Duration(n), nil
}

// nilIfEmpty keeps a nil *Error from turning into a non-nil error interface.
func nilIfEmpty(err *Error) error {
	if err == nil {
		return nil
	}
	return err
}
