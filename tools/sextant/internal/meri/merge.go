package meri

// Merge layers override over base as section 14 of the specification
// describes and returns a new document. Neither input is changed. Values
// are not copied, so lists in the result share memory with the inputs.
func Merge(base, override *Block) *Block {
	m := merger{
		base:     base,
		override: override,
		out:      &Block{Name: base.Name, Label: base.Label, Labelled: base.Labelled, Pos: base.Pos},
		byName:   map[string][]int{},
		used:     make([]bool, len(override.Items)),
		replaced: map[string]bool{},
		seen:     map[string]int{},
	}
	for i, it := range override.Items {
		m.byName[it.Name] = append(m.byName[it.Name], i)
	}
	for _, it := range base.Items {
		m.item(it)
	}
	for k, it := range override.Items {
		if !m.used[k] {
			m.out.add(copyItem(it))
		}
	}
	return m.out
}

type merger struct {
	base, override, out *Block
	byName              map[string][]int
	used                []bool
	replaced            map[string]bool // names whose base items an entry replaced
	seen                map[string]int  // unlabelled base blocks visited per name
}

func (m *merger) take(k int) {
	m.out.add(copyItem(m.override.Items[k]))
	m.used[k] = true
}

func (m *merger) item(it Item) {
	over := m.byName[it.Name]
	switch {
	case len(over) == 0:
		m.out.add(copyItem(it))
	case it.Block == nil || m.override.Items[over[0]].Block == nil:
		// One side has an entry, so the override's items take the place of
		// every base item with this name.
		if !m.replaced[it.Name] {
			for _, k := range over {
				m.take(k)
			}
			m.replaced[it.Name] = true
		}
	case it.Block.Labelled:
		for _, k := range over {
			if b := m.override.Items[k].Block; b.Labelled && b.Label == it.Block.Label {
				m.out.add(Item{Name: it.Name, Pos: it.Pos, Block: Merge(it.Block, b)})
				m.used[k] = true
				return
			}
		}
		m.out.add(copyItem(it))
	default:
		m.unlabelled(it, over)
	}
}

// unlabelled handles a base block without a label. With exactly one
// unlabelled block of the name on each side the two merge. Otherwise the
// override's unlabelled blocks take the places of the base's in order.
func (m *merger) unlabelled(it Item, over []int) {
	var theirs []int
	for _, k := range over {
		if !m.override.Items[k].Block.Labelled {
			theirs = append(theirs, k)
		}
	}
	if len(theirs) == 0 {
		m.out.add(copyItem(it))
		return
	}
	ours := 0
	for _, b := range m.base.Blocks(it.Name) {
		if !b.Labelled {
			ours++
		}
	}
	if ours == 1 && len(theirs) == 1 {
		m.out.add(Item{Name: it.Name, Pos: it.Pos, Block: Merge(it.Block, m.override.Items[theirs[0]].Block)})
		m.used[theirs[0]] = true
		return
	}
	n := m.seen[it.Name]
	m.seen[it.Name] = n + 1
	if n < len(theirs) {
		m.take(theirs[n])
	}
	if n == ours-1 {
		for _, k := range theirs[min(n+1, len(theirs)):] {
			m.take(k)
		}
	}
}

func copyItem(it Item) Item {
	if it.Block != nil {
		it.Block = copyBlock(it.Block)
	}
	return it
}

func copyBlock(b *Block) *Block {
	out := &Block{Name: b.Name, Label: b.Label, Labelled: b.Labelled, Pos: b.Pos}
	for _, it := range b.Items {
		out.add(copyItem(it))
	}
	return out
}
