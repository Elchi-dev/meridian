// Package lint checks the rules of the Meridian repository that no compiler
// or formatter knows about.
package lint

import (
	"bytes"
	"cmp"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"io/fs"
	"path"
	"slices"
	"strings"
	"unicode/utf8"

	"github.com/Elchi-dev/meridian/tools/sextant/internal/srclex"
)

// Finding is one broken rule at one place.
type Finding struct {
	Path string
	Line int
	Col  int
	Rule string
	Msg  string
}

func (f Finding) String() string {
	return fmt.Sprintf("%s:%d:%d: %s: %s", f.Path, f.Line, f.Col, f.Rule, f.Msg)
}

var skipDirs = map[string]bool{".git": true, "target": true, "out": true, "node_modules": true}

var binaryExts = map[string]bool{
	".png": true, ".jpg": true, ".efi": true, ".ttf": true, ".otf": true,
	".fd": true, ".img": true, ".iso": true, ".bin": true, ".der": true,
}

// Check walks fsys and returns every finding, sorted by path and position.
func Check(fsys fs.FS) ([]Finding, error) {
	allowed, err := allowances(fsys)
	if err != nil {
		return nil, err
	}
	var out []Finding
	err = fs.WalkDir(fsys, ".", func(p string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() {
			if skipDirs[d.Name()] {
				return fs.SkipDir
			}
			return nil
		}
		data, err := fs.ReadFile(fsys, p)
		if err != nil {
			return err
		}
		if strings.HasPrefix(p, "third-party/") {
			out = append(out, recorded(p, data, allowed)...)
		}
		if binaryExts[strings.ToLower(path.Ext(p))] {
			return nil
		}
		if bytes.IndexByte(data[:min(len(data), 8000)], 0) >= 0 {
			return nil
		}
		out = append(out, dashes(p, data)...)
		found, err := codeRules(p, data)
		if err != nil {
			return err
		}
		out = append(out, found...)
		out = append(out, dependencies(p, data, allowed)...)
		return nil
	})
	if err != nil {
		return nil, err
	}
	slices.SortFunc(out, func(a, b Finding) int {
		return cmp.Or(strings.Compare(a.Path, b.Path), cmp.Compare(a.Line, b.Line), cmp.Compare(a.Col, b.Col))
	})
	return out, nil
}

// position turns a byte offset into a 1-based line and byte column.
func position(data []byte, off int) (int, int) {
	line := 1 + bytes.Count(data[:off], []byte{'\n'})
	start := bytes.LastIndexByte(data[:off], '\n') + 1
	return line, off - start + 1
}

// dashRunes are the dash characters the project never uses. A comma, a
// colon or a full stop always reads better, and a hyphen covers ranges.
var dashRunes = map[rune]string{
	'\u2012': "figure dash",
	'\u2013': "en dash",
	'\u2014': "em dash",
	'\u2015': "horizontal bar",
	'\u2e3a': "two-em dash",
	'\u2e3b': "three-em dash",
}

func dashes(p string, data []byte) []Finding {
	var out []Finding
	for off := 0; off < len(data); {
		r, size := utf8.DecodeRune(data[off:])
		if name, ok := dashRunes[r]; ok {
			line, col := position(data, off)
			out = append(out, Finding{Path: p, Line: line, Col: col, Rule: "dash",
				Msg: name + "; use a comma, a colon or a full stop, or a hyphen in a range"})
		}
		off += size
	}
	return out
}

var markers = []string{"TODO", "FIXME", "XXX", "HACK"}

// codeRules checks source files: no placeholders and no unsafe outside raw
// modules.
func codeRules(p string, data []byte) ([]Finding, error) {
	lang, ok := srclex.LangOf(p)
	if !ok {
		return nil, nil
	}
	toks, err := srclex.Lex(lang, data)
	if err != nil {
		return nil, fmt.Errorf("%s:%w", p, err)
	}
	var out []Finding
	for i, t := range toks {
		if t.Kind == srclex.Comment {
			for _, m := range markers {
				if at, ok := wordIndex(t.Text, m); ok {
					line, col := position(data, t.Start+at)
					out = append(out, Finding{Path: p, Line: line, Col: col, Rule: "marker",
						Msg: m + " left in a comment; finish the work or write it down in TODO.md"})
				}
			}
			continue
		}
		if lang != srclex.Rust {
			continue
		}
		next := ""
		if i+1 < len(toks) {
			next = toks[i+1].Text
		}
		switch {
		case (t.Text == "todo" || t.Text == "unimplemented") && next == "!":
			out = append(out, Finding{Path: p, Line: t.Line, Col: t.Start - lineStart(data, t.Start) + 1, Rule: "stub",
				Msg: t.Text + "!() is a placeholder; write the code or leave the feature out"})
		case t.Text == "unsafe" && !isRawModule(p):
			out = append(out, Finding{Path: p, Line: t.Line, Col: t.Start - lineStart(data, t.Start) + 1, Rule: "unsafe",
				Msg: "unsafe code belongs in a raw module (raw.rs or raw/); move it there behind a safe function"})
		}
	}
	return out, nil
}

func lineStart(data []byte, off int) int {
	return bytes.LastIndexByte(data[:off], '\n') + 1
}

// wordIndex finds word in s where it is not part of a longer word.
func wordIndex(s, word string) (int, bool) {
	for from := 0; ; {
		at := strings.Index(s[from:], word)
		if at < 0 {
			return 0, false
		}
		at += from
		end := at + len(word)
		if (at == 0 || !isWordByte(s[at-1])) && (end == len(s) || !isWordByte(s[end])) {
			return at, true
		}
		from = at + 1
	}
}

func isWordByte(c byte) bool {
	return c == '_' || (c >= '0' && c <= '9') || (c|0x20 >= 'a' && c|0x20 <= 'z')
}

func isRawModule(p string) bool {
	for _, part := range strings.Split(p, "/") {
		if part == "raw" || part == "raw.rs" {
			return true
		}
	}
	return false
}

// allowance is the decision record that lets something in.
type allowance struct {
	record, text string
}

// allowances reads every accepted decision record and collects what its
// "- Allows:" lines name: crates, Go modules, CI actions or third-party files.
func allowances(fsys fs.FS) (map[string]allowance, error) {
	allowed := map[string]allowance{}
	records, err := fs.Glob(fsys, "docs/adr/[0-9][0-9][0-9][0-9]-*.md")
	if err != nil {
		return nil, err
	}
	for _, rec := range records {
		data, err := fs.ReadFile(fsys, rec)
		if err != nil {
			return nil, err
		}
		accepted := false
		var allows []string
		for _, line := range strings.Split(string(data), "\n") {
			line = strings.TrimSpace(line)
			if v, ok := strings.CutPrefix(line, "- Status:"); ok {
				accepted = strings.TrimSpace(v) == "accepted"
			}
			if v, ok := strings.CutPrefix(line, "- Allows:"); ok {
				for _, item := range strings.Split(v, ",") {
					if item = strings.Trim(strings.TrimSpace(item), "`"); item != "" {
						allows = append(allows, item)
					}
				}
			}
		}
		if accepted {
			for _, a := range allows {
				allowed[a] = allowance{record: rec, text: string(data)}
			}
		}
	}
	return allowed, nil
}

// recorded checks that the third-party file p, holding data, has a record
// that allows it and names its SHA-256, so a file that changes without its
// record is caught.
func recorded(p string, data []byte, allowed map[string]allowance) []Finding {
	a, ok := allowed[p]
	if !ok {
		return []Finding{{Path: p, Line: 1, Col: 1, Rule: "dependency",
			Msg: "outside file without a decision record; add one in docs/adr with \"- Allows: " + p + "\""}}
	}
	sum := sha256.Sum256(data)
	if digest := hex.EncodeToString(sum[:]); !strings.Contains(a.text, digest) {
		return []Finding{{Path: p, Line: 1, Col: 1, Rule: "dependency",
			Msg: "its SHA-256 " + digest + " is not in " + a.record + ", which allows it; a file that changes needs its record changed with it"}}
	}
	return nil
}

// dependencies finds outside code: registry or git packages in Cargo.lock,
// required modules in go.mod, and actions used by workflows and composite
// actions under .github.
func dependencies(p string, data []byte, allowed map[string]allowance) []Finding {
	base := path.Base(p)
	lines := strings.Split(string(data), "\n")
	var out []Finding
	report := func(i int, name, kind string) {
		if _, ok := allowed[name]; ok {
			return
		}
		out = append(out, Finding{Path: p, Line: i + 1, Col: 1, Rule: "dependency",
			Msg: kind + " " + name + " has no decision record; add one in docs/adr with \"- Allows: " + name + "\" or remove it"})
	}
	switch {
	case base == "Cargo.lock":
		name, nameLine := "", 0
		for i, line := range lines {
			if v, ok := strings.CutPrefix(line, "name = "); ok {
				name, nameLine = strings.Trim(v, `"`), i
			}
			if strings.HasPrefix(line, "source = ") {
				report(nameLine, name, "crate")
			}
		}
	case base == "go.mod":
		inBlock := false
		for i, line := range lines {
			line = strings.TrimSpace(line)
			switch {
			case line == ")":
				inBlock = false
			case inBlock && line != "" && !strings.HasPrefix(line, "//"):
				report(i, strings.Fields(line)[0], "module")
			case strings.HasPrefix(line, "require"):
				rest := strings.TrimSpace(strings.TrimPrefix(line, "require"))
				if rest == "(" {
					inBlock = true
				} else if fields := strings.Fields(rest); len(fields) > 0 {
					report(i, fields[0], "module")
				}
			}
		}
	case strings.HasPrefix(p, ".github/") && (strings.HasSuffix(p, ".yml") || strings.HasSuffix(p, ".yaml")):
		for i, line := range lines {
			v, ok := strings.CutPrefix(strings.TrimSpace(strings.TrimPrefix(strings.TrimSpace(line), "-")), "uses:")
			if !ok {
				continue
			}
			action, _, _ := strings.Cut(strings.Trim(strings.TrimSpace(v), `"'`), "@")
			if !strings.HasPrefix(action, "./") {
				report(i, action, "action")
			}
		}
	}
	return out
}
