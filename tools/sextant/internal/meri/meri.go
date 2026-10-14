// Package meri reads .meri documents, the format Meridian uses for its
// configuration and system files. docs/formats/meri.md is the specification.
package meri

import "strconv"

// Pos is where something starts in a document. Lines and columns count from
// 1, and columns count characters, not bytes.
type Pos struct {
	File string
	Line int
	Col  int
}

func (p Pos) String() string {
	return p.File + ":" + strconv.Itoa(p.Line) + ":" + strconv.Itoa(p.Col)
}

// Kind is the type of a value.
type Kind uint8

const (
	KindBool Kind = iota + 1
	KindInt
	KindFloat
	KindQuantity
	KindString
	KindWord
	KindList
)

var kindNames = [...]string{
	KindBool:     "bool",
	KindInt:      "int",
	KindFloat:    "float",
	KindQuantity: "quantity",
	KindString:   "string",
	KindWord:     "word",
	KindList:     "list",
}

// String returns the name the canonical dump uses for the kind.
func (k Kind) String() string {
	if int(k) < len(kindNames) && kindNames[k] != "" {
		return kindNames[k]
	}
	return "kind(" + strconv.Itoa(int(k)) + ")"
}

// Value is one value of a document. Which fields are set depends on Kind:
// Bool for booleans, Int for integers and for quantities written as integers,
// Float for floats and for quantities written as floats, Num and Unit for
// quantities, Str for strings and words, List for lists. Num is set for every
// number and holds it the way the canonical dump writes it.
type Value struct {
	Kind  Kind
	Pos   Pos
	Bool  bool
	Int   int64
	Float float64
	Num   string
	Unit  string
	Str   string
	List  []Value
}

// Item is an entry (Block is nil, Value is set) or a nested block.
type Item struct {
	Name  string
	Pos   Pos
	Value Value
	Block *Block
}

// Block is the root of a document or a block inside it.
type Block struct {
	Name     string
	Label    string
	Labelled bool
	Pos      Pos
	Items    []Item

	// index maps a name to its first item, and labels a labelled block to
	// its item, once a block holds more than a handful of items. Without
	// them, large documents would check duplicates in quadratic time.
	index  map[string]int
	labels map[labelKey]int
}

type labelKey struct{ name, label string }

const indexAfter = 16

func (b *Block) add(it Item) {
	b.Items = append(b.Items, it)
	b.added()
}

// added updates the indexes after an item was appended to b.Items.
func (b *Block) added() {
	if b.index == nil {
		if len(b.Items) <= indexAfter {
			return
		}
		b.index = map[string]int{}
		b.labels = map[labelKey]int{}
		for i := range b.Items {
			b.indexItem(i)
		}
		return
	}
	b.indexItem(len(b.Items) - 1)
}

// indexItem records item i unless an earlier item already has its name.
// Labels are unique, so a labelled block is always recorded.
func (b *Block) indexItem(i int) {
	it := b.Items[i]
	if _, ok := b.index[it.Name]; !ok {
		b.index[it.Name] = i
	}
	if it.Block != nil && it.Block.Labelled {
		b.labels[labelKey{it.Name, it.Block.Label}] = i
	}
}

// labelled returns the item of the block with the name and label, or -1.
func (b *Block) labelled(name, label string) int {
	if b.labels != nil {
		if i, ok := b.labels[labelKey{name, label}]; ok {
			return i
		}
		return -1
	}
	for i, it := range b.Items {
		if it.Name == name && it.Block != nil && it.Block.Labelled && it.Block.Label == label {
			return i
		}
	}
	return -1
}

// find returns the index of the first item with the name, or -1.
func (b *Block) find(name string) int {
	if b.index != nil {
		if i, ok := b.index[name]; ok {
			return i
		}
		return -1
	}
	for i := range b.Items {
		if b.Items[i].Name == name {
			return i
		}
	}
	return -1
}

// Get returns the value of the entry with the key. It reports false when
// there is no such entry, including when the name belongs to blocks.
func (b *Block) Get(key string) (Value, bool) {
	i := b.find(key)
	if i < 0 || b.Items[i].Block != nil {
		return Value{}, false
	}
	return b.Items[i].Value, true
}

// Blocks returns the blocks with the name in document order.
func (b *Block) Blocks(name string) []*Block {
	i := b.find(name)
	if i < 0 {
		return nil
	}
	var out []*Block
	for _, it := range b.Items[i:] {
		if it.Name == name && it.Block != nil {
			out = append(out, it.Block)
		}
	}
	return out
}

// LabelledBlock returns the block with the name and label.
func (b *Block) LabelledBlock(name, label string) (*Block, bool) {
	if i := b.labelled(name, label); i >= 0 {
		return b.Items[i].Block, true
	}
	return nil, false
}
