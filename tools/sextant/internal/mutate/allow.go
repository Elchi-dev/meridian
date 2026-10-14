package mutate

import (
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"strings"

	"github.com/Elchi-dev/meridian/tools/sextant/internal/meri"
)

// AllowFile is the allow list at the repository root.
const AllowFile = "mutants.meri"

// Allowance is one mutant no test can kill because it cannot change what
// the code does, with the reason why.
type Allowance struct {
	File     string
	Line     string
	Mutation string
	Reason   string
	Pos      meri.Pos
}

var allowKeys = []string{"file", "line", "mutation", "reason"}

// ParseAllowances reads an allow list: allow blocks and nothing else, each
// with exactly the keys file, line, mutation and reason.
func ParseAllowances(name string, src []byte) ([]Allowance, error) {
	doc, err := meri.Parse(name, src)
	if err != nil {
		return nil, err
	}
	var out []Allowance
	for _, it := range doc.Items {
		if it.Name != "allow" || it.Block == nil {
			return nil, fmt.Errorf("%s: only allow blocks belong in %s, not %q", it.Pos, AllowFile, it.Name)
		}
		if it.Block.Labelled {
			return nil, fmt.Errorf("%s: allow takes no label", it.Pos)
		}
		for _, field := range it.Block.Items {
			if field.Block != nil || !contains(allowKeys, field.Name) {
				return nil, fmt.Errorf("%s: an allow has only file, line, mutation and reason, not %q", field.Pos, field.Name)
			}
		}
		a := Allowance{Pos: it.Pos}
		for _, f := range []struct {
			key string
			to  *string
		}{{"file", &a.File}, {"line", &a.Line}, {"mutation", &a.Mutation}, {"reason", &a.Reason}} {
			v, err := it.Block.String(f.key, "")
			if err != nil {
				return nil, err
			}
			if v == "" {
				return nil, fmt.Errorf("%s: this allow has no %s; every allow needs file, line, mutation and reason", it.Pos, f.key)
			}
			*f.to = v
		}
		out = append(out, a)
	}
	return out, nil
}

func contains(list []string, s string) bool {
	for _, x := range list {
		if x == s {
			return true
		}
	}
	return false
}

// LoadAllowances reads the allow list of the repository at root. A
// repository without one allows nothing.
func LoadAllowances(root string) ([]Allowance, error) {
	src, err := os.ReadFile(filepath.Join(root, AllowFile))
	if errors.Is(err, fs.ErrNotExist) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	return ParseAllowances(AllowFile, src)
}

func (a Allowance) matches(m Mutant) bool {
	return a.File == m.Path && a.Line == m.LineText && a.Mutation == m.Desc
}

// Split separates the mutants an allowance covers from the ones to run.
// Allowances for checked files that match no mutant are stale: the code
// they describe has changed or gone.
func Split(allow []Allowance, mutants []Mutant, checked map[string]bool) (run, allowed []Mutant, stale []Allowance) {
	used := make([]bool, len(allow))
	for _, m := range mutants {
		covered := false
		for i, a := range allow {
			if a.matches(m) {
				used[i] = true
				covered = true
			}
		}
		if covered {
			allowed = append(allowed, m)
		} else {
			run = append(run, m)
		}
	}
	for i, a := range allow {
		if !used[i] && checked[a.File] {
			stale = append(stale, a)
		}
	}
	return run, allowed, stale
}

// AllowEntry is the allow block for the mutant, ready to paste into
// mutants.meri once the reason is filled in.
func (m Mutant) AllowEntry() string {
	return "allow {\n" +
		"    file = " + meriString(m.Path) + "\n" +
		"    line = " + meriString(m.LineText) + "\n" +
		"    mutation = " + meriString(m.Desc) + "\n" +
		"    reason = \"why this change cannot alter behaviour\"\n" +
		"}\n"
}

// meriString writes s as a .meri string, escaping what the format needs.
func meriString(s string) string {
	var b strings.Builder
	b.WriteByte('"')
	for _, r := range s {
		switch {
		case r == '"' || r == '\\':
			b.WriteByte('\\')
			b.WriteRune(r)
		case r == '\t':
			b.WriteString(`\t`)
		case r < 0x20 || r == 0x7f:
			fmt.Fprintf(&b, `\u{%x}`, r)
		default:
			b.WriteRune(r)
		}
	}
	b.WriteByte('"')
	return b.String()
}
