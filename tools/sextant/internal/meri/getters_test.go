package meri

import (
	"errors"
	"reflect"
	"testing"
	"time"
)

const getterDoc = `name = "Inter"
accel = kvm
cpus = 4
ratio = 1.5
verbose = true
quiet = false
memory = 512MiB
timeout = 30s
tags = [fast, "safe", x]
mixed = ["a", 4]
qemu { cpus = 2 }
quoted = "a\"b"
`

func getterBlock(t *testing.T) *Block {
	t.Helper()
	doc, err := Parse("cfg.meri", []byte(getterDoc))
	if err != nil {
		t.Fatal(err)
	}
	return doc
}

// wantError checks that err is an *Error with the exact message, code and
// position. Callers pass the expected Error() text.
func wantError(t *testing.T, err error, code, text string) {
	t.Helper()
	var e *Error
	if !errors.As(err, &e) {
		t.Fatalf("got %v (%T), want an *Error", err, err)
	}
	if e.Code != code || e.Error() != text {
		t.Errorf("got %s %q\nwant %s %q", e.Code, e.Error(), code, text)
	}
}

func TestMissingKeysReturnTheDefaultWithoutError(t *testing.T) {
	b := getterBlock(t)
	if s, err := b.String("nope", "def"); s != "def" || err != nil {
		t.Errorf("String: %q, %v", s, err)
	}
	if n, err := b.Int("nope", 7); n != 7 || err != nil {
		t.Errorf("Int: %d, %v", n, err)
	}
	if f, err := b.Float("nope", 2.5); f != 2.5 || err != nil {
		t.Errorf("Float: %v, %v", f, err)
	}
	if v, err := b.Bool("nope", true); !v || err != nil {
		t.Errorf("Bool: %v, %v", v, err)
	}
	if n, err := b.Size("nope", 99); n != 99 || err != nil {
		t.Errorf("Size: %d, %v", n, err)
	}
	if d, err := b.Duration("nope", time.Minute); d != time.Minute || err != nil {
		t.Errorf("Duration: %v, %v", d, err)
	}
	if l, err := b.Strings("nope", []string{"d"}); !reflect.DeepEqual(l, []string{"d"}) || err != nil {
		t.Errorf("Strings: %v, %v", l, err)
	}
}

func TestPresentValuesAreReturned(t *testing.T) {
	b := getterBlock(t)
	if s, err := b.String("name", ""); s != "Inter" || err != nil {
		t.Errorf("String(name): %q, %v", s, err)
	}
	if s, err := b.String("accel", ""); s != "kvm" || err != nil {
		t.Errorf("String(accel) should accept a word: %q, %v", s, err)
	}
	if n, err := b.Int("cpus", 0); n != 4 || err != nil {
		t.Errorf("Int: %d, %v", n, err)
	}
	if f, err := b.Float("ratio", 0); f != 1.5 || err != nil {
		t.Errorf("Float: %v, %v", f, err)
	}
	if f, err := b.Float("cpus", 0); f != 4 || err != nil {
		t.Errorf("Float should accept an integer: %v, %v", f, err)
	}
	if v, err := b.Bool("verbose", false); !v || err != nil {
		t.Errorf("Bool(verbose): %v, %v", v, err)
	}
	if v, err := b.Bool("quiet", true); v || err != nil {
		t.Errorf("Bool(quiet): %v, %v", v, err)
	}
	if n, err := b.Size("memory", 0); n != 512<<20 || err != nil {
		t.Errorf("Size: %d, %v", n, err)
	}
	if d, err := b.Duration("timeout", 0); d != 30*time.Second || err != nil {
		t.Errorf("Duration: %v, %v", d, err)
	}
	if l, err := b.Strings("tags", nil); !reflect.DeepEqual(l, []string{"fast", "safe", "x"}) || err != nil {
		t.Errorf("Strings: %v, %v", l, err)
	}
}

func TestWrongTypesNameTheValue(t *testing.T) {
	b := getterBlock(t)
	tests := []struct {
		name string
		get  func() error
		text string
	}{
		{"string from number", func() error { _, err := b.String("cpus", ""); return err },
			`cfg.meri:3:8: cpus must be text such as "Inter" or a word such as kvm, found the number 4`},
		{"int from float", func() error { _, err := b.Int("ratio", 0); return err },
			"cfg.meri:4:9: ratio must be a whole number such as 4, found the number 1.5"},
		{"int from word", func() error { _, err := b.Int("accel", 0); return err },
			"cfg.meri:2:9: accel must be a whole number such as 4, found the word kvm"},
		{"float from string", func() error { _, err := b.Float("name", 0); return err },
			`cfg.meri:1:8: name must be a number such as 1.5, found the string "Inter"`},
		{"bool from int", func() error { _, err := b.Bool("cpus", false); return err },
			"cfg.meri:3:8: cpus must be true or false, found the number 4"},
		{"size from duration", func() error { _, err := b.Size("timeout", 0); return err },
			"cfg.meri:8:11: timeout must be a size such as 512MiB, found the quantity 30s"},
		{"size from bool", func() error { _, err := b.Size("verbose", 0); return err },
			"cfg.meri:5:11: verbose must be a size such as 512MiB, found true"},
		{"duration from size", func() error { _, err := b.Duration("memory", 0); return err },
			"cfg.meri:7:10: memory must be a duration such as 30s, found the quantity 512MiB"},
		{"duration from false", func() error { _, err := b.Duration("quiet", 0); return err },
			"cfg.meri:6:9: quiet must be a duration such as 30s, found false"},
		{"strings from string", func() error { _, err := b.Strings("name", nil); return err },
			`cfg.meri:1:8: name must be a list of text such as ["a", "b"], found the string "Inter"`},
		{"string from list", func() error { _, err := b.String("tags", ""); return err },
			`cfg.meri:9:8: tags must be text such as "Inter" or a word such as kvm, found a list`},
		{"string with escapes is quoted", func() error { _, err := b.Int("quoted", 0); return err },
			`cfg.meri:12:10: quoted must be a whole number such as 4, found the string "a\"b"`},
		{"value from a block", func() error { _, err := b.Int("qemu", 0); return err },
			"cfg.meri:11:1: qemu must be a whole number such as 4, found a block"},
		{"list item of the wrong type", func() error { _, err := b.Strings("mixed", nil); return err },
			`cfg.meri:10:15: mixed must be a list of text such as ["a", "b"], but item 2 is the number 4`},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) { wantError(t, tt.get(), CodeWrongType, tt.text) })
	}
}

func TestWrongTypesReturnTheDefault(t *testing.T) {
	b := getterBlock(t)
	if s, _ := b.String("cpus", "def"); s != "def" {
		t.Errorf("String = %q", s)
	}
	if n, _ := b.Int("ratio", 3); n != 3 {
		t.Errorf("Int = %d", n)
	}
	if f, _ := b.Float("name", 0.5); f != 0.5 {
		t.Errorf("Float = %v", f)
	}
	if v, _ := b.Bool("cpus", true); !v {
		t.Errorf("Bool = %v", v)
	}
	if n, _ := b.Size("timeout", 8); n != 8 {
		t.Errorf("Size = %d", n)
	}
	if d, _ := b.Duration("memory", time.Hour); d != time.Hour {
		t.Errorf("Duration = %v", d)
	}
	if l, _ := b.Strings("mixed", []string{"d"}); !reflect.DeepEqual(l, []string{"d"}) {
		t.Errorf("Strings = %v", l)
	}
	for _, get := range []func() error{
		func() error { _, err := b.String("qemu", ""); return err },
		func() error { _, err := b.Float("qemu", 0); return err },
		func() error { _, err := b.Bool("qemu", false); return err },
		func() error { _, err := b.Size("qemu", 0); return err },
		func() error { _, err := b.Duration("qemu", 0); return err },
		func() error { _, err := b.Strings("qemu", nil); return err },
	} {
		var e *Error
		if err := get(); !errors.As(err, &e) || e.Code != CodeWrongType || e.Line != 11 {
			t.Errorf("reading the block qemu as a value gave %v", err)
		}
	}
}

func sizeOf(t *testing.T, literal string) (int64, error) {
	t.Helper()
	doc, err := Parse("s.meri", []byte("v = "+literal+"\n"))
	if err != nil {
		t.Fatalf("%s: %v", literal, err)
	}
	return doc.Size("v", -1)
}

func TestSizeUnitsAreExact(t *testing.T) {
	tests := []struct {
		literal string
		want    int64
	}{
		{"0B", 0},
		{"1B", 1},
		{"1KB", 1000},
		{"1MB", 1000000},
		{"1GB", 1000000000},
		{"1TB", 1000000000000},
		{"1KiB", 1024},
		{"1MiB", 1 << 20},
		{"1GiB", 1 << 30},
		{"1TiB", 1 << 40},
		{"1.5KiB", 1536},
		{"0.5MiB", 512 << 10},
		{"2.5e3B", 2500},
		{"1e-3KB", 1},
		{"9223372036854775807B", 9223372036854775807},
		{"8388607.99999999999TiB", 0},
	}
	for _, tt := range tests {
		got, err := sizeOf(t, tt.literal)
		if tt.literal == "8388607.99999999999TiB" {
			wantError(t, err, CodeOutOfRange, "s.meri:1:5: v must be a whole number of bytes, and 8388607.99999999999TiB is not")
			continue
		}
		if err != nil || got != tt.want {
			t.Errorf("%s = %d, %v; want %d", tt.literal, got, err, tt.want)
		}
	}
}

func TestSizesOutOfRange(t *testing.T) {
	tests := []struct{ literal, text string }{
		{"0.5B", "s.meri:1:5: v must be a whole number of bytes, and 0.5B is not"},
		{"1.0001KB", "s.meri:1:5: v must be a whole number of bytes, and 1.0001KB is not"},
		{"-1KiB", "s.meri:1:5: v cannot be negative"},
		{"8388608TiB", "s.meri:1:5: v is too large; sizes must fit in 64 bits"},
		{"9223372036854775807KB", "s.meri:1:5: v is too large; sizes must fit in 64 bits"},
	}
	for _, tt := range tests {
		got, err := sizeOf(t, tt.literal)
		wantError(t, err, CodeOutOfRange, tt.text)
		if got != -1 {
			t.Errorf("%s returned %d instead of the default", tt.literal, got)
		}
	}
	if _, err := sizeOf(t, "-0B"); err != nil {
		t.Errorf("-0B is zero bytes, got %v", err)
	}
}

func TestSizesWithOtherUnitsAreWrongTypes(t *testing.T) {
	for _, literal := range []string{"512mib", "512Mib", "1s", "5x", "4"} {
		_, err := sizeOf(t, literal)
		var e *Error
		if !errors.As(err, &e) || e.Code != CodeWrongType {
			t.Errorf("%s: got %v, want a wrong-type error", literal, err)
		}
	}
}

func durationOf(t *testing.T, literal string) (time.Duration, error) {
	t.Helper()
	doc, err := Parse("d.meri", []byte("v = "+literal+"\n"))
	if err != nil {
		t.Fatalf("%s: %v", literal, err)
	}
	return doc.Duration("v", -1)
}

func TestDurationUnitsAreExact(t *testing.T) {
	tests := []struct {
		literal string
		want    time.Duration
	}{
		{"1ns", time.Nanosecond},
		{"1us", time.Microsecond},
		{"1ms", time.Millisecond},
		{"1s", time.Second},
		{"1min", time.Minute},
		{"1h", time.Hour},
		{"1d", 24 * time.Hour},
		{"1.5h", 90 * time.Minute},
		{"1e-9s", time.Nanosecond},
		{"1.5e-3s", 1500 * time.Microsecond},
		{"-5s", -5 * time.Second},
		{"0.1ms", 100 * time.Microsecond},
		{"106751d", 106751 * 24 * time.Hour},
	}
	for _, tt := range tests {
		if got, err := durationOf(t, tt.literal); err != nil || got != tt.want {
			t.Errorf("%s = %v, %v; want %v", tt.literal, got, err, tt.want)
		}
	}
}

func TestDurationsOutOfRange(t *testing.T) {
	tests := []struct{ literal, text string }{
		{"0.5ns", "d.meri:1:5: v must be a whole number of nanoseconds, and 0.5ns is not"},
		{"1e-10s", "d.meri:1:5: v must be a whole number of nanoseconds, and 1e-10s is not"},
		{"106752d", "d.meri:1:5: v is too long; durations must fit in 64 bits of nanoseconds"},
		{"-106752d", "d.meri:1:5: v is too long; durations must fit in 64 bits of nanoseconds"},
	}
	for _, tt := range tests {
		got, err := durationOf(t, tt.literal)
		wantError(t, err, CodeOutOfRange, tt.text)
		if got != -1 {
			t.Errorf("%s returned %v instead of the default", tt.literal, got)
		}
	}
	for _, literal := range []string{"1sec", "1S", "1m", "5"} {
		_, err := durationOf(t, literal)
		var e *Error
		if !errors.As(err, &e) || e.Code != CodeWrongType {
			t.Errorf("%s: got %v, want a wrong-type error", literal, err)
		}
	}
}

// Values can be built by hand, so a broken number must give an error and
// never a panic.
func TestHandBuiltQuantityWithBrokenNumber(t *testing.T) {
	b := &Block{}
	b.add(Item{Name: "s", Value: Value{Kind: KindQuantity, Num: "lots", Unit: "MiB", Pos: Pos{File: "h", Line: 1, Col: 1}}})
	b.add(Item{Name: "d", Value: Value{Kind: KindQuantity, Num: "lots", Unit: "s", Pos: Pos{File: "h", Line: 2, Col: 1}}})
	if _, err := b.Size("s", 0); err == nil {
		t.Error("Size accepted a broken number")
	}
	if _, err := b.Duration("d", 0); err == nil {
		t.Error("Duration accepted a broken number")
	}
}

func TestDescribeEveryKind(t *testing.T) {
	tests := []struct {
		v    Value
		want string
	}{
		{Value{Kind: KindBool, Bool: true}, "true"},
		{Value{Kind: KindBool}, "false"},
		{Value{Kind: KindInt, Num: "-3"}, "the number -3"},
		{Value{Kind: KindFloat, Num: "2e3"}, "the number 2e3"},
		{Value{Kind: KindQuantity, Num: "4", Unit: "KiB"}, "the quantity 4KiB"},
		{Value{Kind: KindString, Str: "a\nb"}, `the string "a\nb"`},
		{Value{Kind: KindWord, Str: "kvm"}, "the word kvm"},
		{Value{Kind: KindList}, "a list"},
		{Value{}, "a value of kind kind(0)"},
	}
	for _, tt := range tests {
		if got := describe(tt.v); got != tt.want {
			t.Errorf("describe(%+v) = %q, want %q", tt.v, got, tt.want)
		}
	}
}

func TestNilIfEmptyKeepsNilErrorsNil(t *testing.T) {
	if err := nilIfEmpty(nil); err != nil {
		t.Fatalf("nilIfEmpty(nil) = %#v, want a nil error interface", err)
	}
	e := &Error{Msg: "x"}
	if err := nilIfEmpty(e); err != e {
		t.Fatalf("nilIfEmpty changed the error")
	}
}

func TestEveryGetterRejectsEveryOtherKind(t *testing.T) {
	b := getterBlock(t)
	tests := []struct {
		getter string
		get    func(key string) error
		accept map[string]bool
	}{
		{"String", func(k string) error { _, err := b.String(k, ""); return err }, map[string]bool{"name": true, "accel": true, "quoted": true}},
		{"Int", func(k string) error { _, err := b.Int(k, 0); return err }, map[string]bool{"cpus": true}},
		{"Float", func(k string) error { _, err := b.Float(k, 0); return err }, map[string]bool{"cpus": true, "ratio": true}},
		{"Bool", func(k string) error { _, err := b.Bool(k, false); return err }, map[string]bool{"verbose": true, "quiet": true}},
		{"Size", func(k string) error { _, err := b.Size(k, 0); return err }, map[string]bool{"memory": true}},
		{"Duration", func(k string) error { _, err := b.Duration(k, 0); return err }, map[string]bool{"timeout": true}},
		{"Strings", func(k string) error { _, err := b.Strings(k, nil); return err }, map[string]bool{"tags": true}},
	}
	keys := []string{"name", "accel", "cpus", "ratio", "verbose", "quiet", "memory", "timeout", "tags", "mixed", "qemu", "quoted"}
	for _, tt := range tests {
		for _, k := range keys {
			err := tt.get(k)
			if tt.accept[k] {
				if err != nil {
					t.Errorf("%s(%s) failed: %v", tt.getter, k, err)
				}
				continue
			}
			var e *Error
			if !errors.As(err, &e) || e.Code != CodeWrongType {
				t.Errorf("%s(%s) = %v, want a wrong-type error", tt.getter, k, err)
			}
		}
	}
}

// Values can be built by hand. A unit on a value that is not a quantity
// must not turn it into a size or a duration.
func TestUnitsOnlyCountOnQuantities(t *testing.T) {
	b := &Block{}
	b.add(Item{Name: "s", Value: Value{Kind: KindInt, Int: 5, Num: "5", Unit: "MiB"}})
	b.add(Item{Name: "d", Value: Value{Kind: KindFloat, Float: 1.5, Num: "1.5", Unit: "s"}})
	var e *Error
	if _, err := b.Size("s", 0); !errors.As(err, &e) || e.Code != CodeWrongType {
		t.Errorf("Size of an int carrying a unit: %v", err)
	}
	if _, err := b.Duration("d", 0); !errors.As(err, &e) || e.Code != CodeWrongType {
		t.Errorf("Duration of a float carrying a unit: %v", err)
	}
}

func TestStringsOfAnEmptyList(t *testing.T) {
	doc := mustParse(t, "none = []\n")
	got, err := doc.Strings("none", []string{"default"})
	if err != nil || got == nil || len(got) != 0 {
		t.Fatalf("Strings of [] = %#v, %v; want an empty, non-nil list", got, err)
	}
}

func TestStringsNamesTheItemInANestedList(t *testing.T) {
	doc := mustParse(t, "l = [\"a\",\n  [\"b\"]]\n")
	_, err := doc.Strings("l", nil)
	wantError(t, err, CodeWrongType, `test.meri:2:3: l must be a list of text such as ["a", "b"], but item 2 is a list`)
}
