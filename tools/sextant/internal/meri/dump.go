package meri

import (
	"strconv"
	"strings"
)

// Dump writes a document in the canonical form of section 15 of the
// specification: one line per item, depth first, the form the conformance
// suite compares.
func Dump(b *Block) string {
	var w strings.Builder
	dumpBlock(&w, "", b)
	return w.String()
}

func dumpBlock(w *strings.Builder, prefix string, b *Block) {
	var seen map[string]int
	for _, it := range b.Items {
		path := it.Name
		if prefix != "" {
			path = prefix + "." + it.Name
		}
		if it.Block == nil {
			dumpValue(w, path, it.Value)
			continue
		}
		if seen == nil {
			seen = map[string]int{}
		}
		path += "#" + strconv.Itoa(seen[it.Name])
		seen[it.Name]++
		w.WriteString(path)
		w.WriteString(" block")
		if it.Block.Labelled {
			w.WriteByte(' ')
			w.WriteString(quote(it.Block.Label))
		}
		w.WriteByte('\n')
		dumpBlock(w, path, it.Block)
	}
}

func dumpValue(w *strings.Builder, path string, v Value) {
	w.WriteString(path)
	w.WriteByte(' ')
	w.WriteString(v.Kind.String())
	w.WriteByte(' ')
	switch v.Kind {
	case KindBool:
		w.WriteString(strconv.FormatBool(v.Bool))
	case KindInt:
		w.WriteString(strconv.FormatInt(v.Int, 10))
	case KindFloat:
		w.WriteString(v.Num)
	case KindQuantity:
		w.WriteString(v.Num)
		w.WriteByte(' ')
		w.WriteString(v.Unit)
	case KindString:
		w.WriteString(quote(v.Str))
	case KindWord:
		w.WriteString(v.Str)
	case KindList:
		w.WriteString(strconv.Itoa(len(v.List)))
	}
	w.WriteByte('\n')
	for i, e := range v.List {
		dumpValue(w, path+"["+strconv.Itoa(i)+"]", e)
	}
}

// quote writes a string with the escapes of the canonical dump.
func quote(s string) string {
	var w strings.Builder
	w.Grow(len(s) + 2)
	w.WriteByte('"')
	for i := 0; i < len(s); i++ {
		c := s[i]
		switch {
		case c == '\\':
			w.WriteString(`\\`)
		case c == '"':
			w.WriteString(`\"`)
		case c == '\n':
			w.WriteString(`\n`)
		case c == '\r':
			w.WriteString(`\r`)
		case c == '\t':
			w.WriteString(`\t`)
		case c == 0:
			w.WriteString(`\0`)
		case c < 0x20 || c == 0x7F:
			w.WriteString(`\u{`)
			w.WriteString(strconv.FormatUint(uint64(c), 16))
			w.WriteByte('}')
		default:
			w.WriteByte(c)
		}
	}
	w.WriteByte('"')
	return w.String()
}
