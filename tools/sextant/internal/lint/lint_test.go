package lint

import (
	"strings"
	"testing"
	"testing/fstest"
)

const emDash, enDash = "\u2014", "\u2013"

func check(t *testing.T, files map[string]string) []string {
	t.Helper()
	fsys := fstest.MapFS{}
	for name, body := range files {
		fsys[name] = &fstest.MapFile{Data: []byte(body)}
	}
	found, err := Check(fsys)
	if err != nil {
		t.Fatalf("Check: %v", err)
	}
	out := make([]string, len(found))
	for i, f := range found {
		out[i] = f.String()
	}
	return out
}

func expect(t *testing.T, got []string, want ...string) {
	t.Helper()
	if strings.Join(got, "\n") != strings.Join(want, "\n") {
		t.Fatalf("findings:\n%s\nwant:\n%s", strings.Join(got, "\n"), strings.Join(want, "\n"))
	}
}

const dashMsg = "; use a comma, a colon or a full stop, or a hyphen in a range"

func TestDashesAnywhere(t *testing.T) {
	got := check(t, map[string]string{
		"README.md":            "# Meridian\n\nFast " + emDash + " and small.\n",
		"docs/x.md":            "pages 2" + enDash + "5\n",
		".github/workflows/ci": "name: ci " + emDash + " build\n",
		"tools/a.go":           "package a\n\n// Note \u2012 x\nvar s = \"\u2015\"\n",
	})
	expect(t, got,
		".github/workflows/ci:1:10: dash: em dash"+dashMsg,
		"README.md:3:6: dash: em dash"+dashMsg,
		"docs/x.md:1:8: dash: en dash"+dashMsg,
		"tools/a.go:3:9: dash: figure dash"+dashMsg,
		"tools/a.go:4:10: dash: horizontal bar"+dashMsg,
	)
}

func TestEveryDashKind(t *testing.T) {
	got := check(t, map[string]string{"a.txt": "\u2e3a\u2e3b"})
	expect(t, got, "a.txt:1:1: dash: two-em dash"+dashMsg, "a.txt:1:4: dash: three-em dash"+dashMsg)
}

func TestHyphensAndMinusAreFine(t *testing.T) {
	expect(t, check(t, map[string]string{"a.md": "ports 1100-1999, x \u2212 y, a\u2010b\n"}))
}

func TestBinaryFilesAreSkipped(t *testing.T) {
	expect(t, check(t, map[string]string{
		"assets/logo.png":    emDash,
		"boot/BOOTX64.EFI":   emDash,
		"data/blob":          "\x00" + emDash,
		"third-party/x.ttf":  emDash,
		"docs/adr/0001-a.md": "# a\n\n- Status: accepted\n- Allows: third-party/x.ttf\n",
	}))
}

func TestNulAfterTheFirst8000BytesStillChecks(t *testing.T) {
	body := strings.Repeat("a", 8000) + "\x00\n" + emDash
	got := check(t, map[string]string{"big.txt": body})
	expect(t, got, "big.txt:2:1: dash: em dash"+dashMsg)
}

func TestSkippedDirectories(t *testing.T) {
	expect(t, check(t, map[string]string{
		".git/COMMIT_EDITMSG":       emDash,
		"target/debug/out.txt":      emDash,
		"kernel/target/x.rs":        "fn x() { todo!() }",
		"out/image.txt":             emDash,
		"site/node_modules/a/b.txt": emDash,
	}))
}

func TestMarkersInComments(t *testing.T) {
	got := check(t, map[string]string{
		"k/src/lib.rs":  "// TODO: map the framebuffer\nfn a() {} /* FIXME XXX */\nconst S: &str = \"TODO in a string is fine\";\n// TODOS and todo are not markers\n",
		"t/main.go":     "package main\n\n/*\n   HACK around a bug\n*/\n",
		"k/src/entry.s": "nop # XXX\n",
		"k/link.ld":     "/* TODO */\n",
		"TODO.md":       "# TODO\n\n- TODO: write the scheduler\n",
	})
	msg := " left in a comment; finish the work or write it down in TODO.md"
	expect(t, got,
		"k/link.ld:1:4: marker: TODO"+msg,
		"k/src/entry.s:1:7: marker: XXX"+msg,
		"k/src/lib.rs:1:4: marker: TODO"+msg,
		"k/src/lib.rs:2:14: marker: FIXME"+msg,
		"k/src/lib.rs:2:20: marker: XXX"+msg,
		"t/main.go:4:4: marker: HACK"+msg,
	)
}

func TestStubMacroAtTheVeryEnd(t *testing.T) {
	got := check(t, map[string]string{"k/src/end.rs": "todo!"})
	expect(t, got, "k/src/end.rs:1:1: stub: todo!() is a placeholder; write the code or leave the feature out")
}

func TestStubMacros(t *testing.T) {
	got := check(t, map[string]string{
		"k/src/lib.rs": "fn a() {\n    todo!()\n}\nfn b() { unimplemented!(\"later\") }\nfn c(todo: u8) -> u8 { todo }\n",
		"t/a.go":       "package a\n\nfunc todo() {}\n",
	})
	expect(t, got,
		"k/src/lib.rs:2:5: stub: todo!() is a placeholder; write the code or leave the feature out",
		"k/src/lib.rs:4:10: stub: unimplemented!() is a placeholder; write the code or leave the feature out",
	)
}

func TestUnsafeOnlyInRawModules(t *testing.T) {
	msg := ": unsafe: unsafe code belongs in a raw module (raw.rs or raw/); move it there behind a safe function"
	got := check(t, map[string]string{
		"k/src/raw.rs":          "pub unsafe fn outb() {}\n",
		"k/src/raw/port.rs":     "unsafe impl Send for P {}\n",
		"k/src/arch/raw/msr.rs": "unsafe fn rdmsr() {}\n",
		"k/src/rawish.rs":       "fn f() { unsafe { x() } }\n",
		"k/src/lib.rs":          "mod raw;\n\nunsafe fn bad() {}\n",
		"t/a.go":                "package a\n\nimport \"unsafe\"\n\nvar p unsafe.Pointer\n",
		"k/src/notes.rs":        "// unsafe in a comment\nconst S: &str = \"unsafe\";\n",
	})
	expect(t, got, "k/src/lib.rs:3:1"+msg, "k/src/rawish.rs:1:10"+msg)
}

func TestLexErrorsNameTheFile(t *testing.T) {
	fsys := fstest.MapFS{"k/src/lib.rs": &fstest.MapFile{Data: []byte("fn a() {}\n\"open")}}
	_, err := Check(fsys)
	if err == nil || err.Error() != "k/src/lib.rs:2:1: string literal is never closed" {
		t.Fatalf("got %v", err)
	}
}

const lockWithRegistry = `version = 4

[[package]]
name = "boot"
version = "0.1.0"

[[package]]
name = "spin"
version = "0.9.8"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "x86_64"
version = "0.15.0"
source = "git+https://example.com/x86_64"
`

func TestCratesFromOutside(t *testing.T) {
	msg := " has no decision record; add one in docs/adr with \"- Allows: "
	got := check(t, map[string]string{"Cargo.lock": lockWithRegistry})
	expect(t, got,
		"Cargo.lock:8:1: dependency: crate spin"+msg+"spin\" or remove it",
		"Cargo.lock:13:1: dependency: crate x86_64"+msg+"x86_64\" or remove it",
	)
	got = check(t, map[string]string{
		"Cargo.lock":            lockWithRegistry,
		"docs/adr/0007-spin.md": "# 0007. Use spin\n\n- Status: accepted\n- Allows: `spin`, x86_64\n",
	})
	expect(t, got)
}

func TestOnlyAcceptedRecordsAllow(t *testing.T) {
	for _, status := range []string{"proposed", "replaced by 0009", ""} {
		got := check(t, map[string]string{
			"Cargo.lock":            lockWithRegistry,
			"docs/adr/0007-spin.md": "# 0007\n\n- Status: " + status + "\n- Allows: spin, x86_64\n",
		})
		if len(got) != 2 {
			t.Errorf("status %q allowed dependencies: %v", status, got)
		}
	}
}

func TestOnlyNumberedRecordsCount(t *testing.T) {
	got := check(t, map[string]string{
		"Cargo.lock":         lockWithRegistry,
		"docs/adr/README.md": "- Status: accepted\n- Allows: spin, x86_64\n",
		"docs/adr/draft.md":  "- Status: accepted\n- Allows: spin, x86_64\n",
		"docs/other/0001.md": "- Status: accepted\n- Allows: spin, x86_64\n",
	})
	if len(got) != 2 {
		t.Fatalf("only docs/adr/NNNN-*.md may allow anything, got %v", got)
	}
}

func TestGoModules(t *testing.T) {
	msg := " has no decision record; add one in docs/adr with \"- Allows: "
	got := check(t, map[string]string{
		"tools/x/go.mod":     "module x\n\ngo 1.27.0\n\nrequire golang.org/x/sys v0.1.0\n\nrequire (\n\t// tools only\n\tgithub.com/a/b v1.0.0\n\n\tgithub.com/c/d v2.0.0 // indirect\n)\n",
		"docs/adr/0003-b.md": "- Status: accepted\n- Allows: github.com/a/b\n",
	})
	expect(t, got,
		"tools/x/go.mod:5:1: dependency: module golang.org/x/sys"+msg+"golang.org/x/sys\" or remove it",
		"tools/x/go.mod:11:1: dependency: module github.com/c/d"+msg+"github.com/c/d\" or remove it",
	)
}

func TestGoModRequireWithoutSpace(t *testing.T) {
	got := check(t, map[string]string{"go.mod": "module x\n\nrequire(\n\tgithub.com/a/b v1.0.0\n)\n"})
	if len(got) != 1 || !strings.HasPrefix(got[0], "go.mod:4:1: dependency: module github.com/a/b ") {
		t.Fatalf("got %v", got)
	}
}

func TestGoModWithoutRequirements(t *testing.T) {
	expect(t, check(t, map[string]string{"tools/sextant/go.mod": "module github.com/Elchi-dev/meridian/tools/sextant\n\ngo 1.27.0\n"}))
}

func TestWorkflowActions(t *testing.T) {
	msg := " has no decision record; add one in docs/adr with \"- Allows: "
	got := check(t, map[string]string{
		".github/workflows/ci.yml": "jobs:\n  a:\n    steps:\n      - uses: actions/checkout@v4\n      - uses: \"actions/setup-go@abc123\"\n      - uses: ./.github/local\n      -   uses:   'docker/x@v1'\n      - run: echo uses: nothing\n",
		"docs/adr/0011-ci.md":      "- Status: accepted\n- Allows: actions/checkout\n",
		"docs/notes.yml":           "uses: not/a-workflow@v1\n",
	})
	expect(t, got,
		".github/workflows/ci.yml:5:1: dependency: action actions/setup-go"+msg+"actions/setup-go\" or remove it",
		".github/workflows/ci.yml:7:1: dependency: action docker/x"+msg+"docker/x\" or remove it",
	)
}

func TestCompositeActionsAreCheckedToo(t *testing.T) {
	msg := " has no decision record; add one in docs/adr with \"- Allows: "
	got := check(t, map[string]string{
		".github/actions/setup/action.yaml": "runs:\n  steps:\n    - uses: actions/cache@v4\n    - uses: .hidden/action@v1\n",
		".github/CODEOWNERS":                "uses: not/yaml@v1\n",
	})
	expect(t, got,
		".github/actions/setup/action.yaml:3:1: dependency: action actions/cache"+msg+"actions/cache\" or remove it",
		".github/actions/setup/action.yaml:4:1: dependency: action .hidden/action"+msg+".hidden/action\" or remove it",
	)
}

func TestThirdPartyFilesNeedARecord(t *testing.T) {
	got := check(t, map[string]string{
		"third-party/fonts/Inter.ttf":  "binary",
		"third-party/shim/shimx64.efi": "binary",
		"third-party/README.txt":       "where these come from\n",
		"docs/adr/0012-fonts.md":       "- Status: accepted\n- Allows: third-party/fonts/Inter.ttf, third-party/README.txt\n",
	})
	expect(t, got, "third-party/shim/shimx64.efi:1:1: dependency: outside file without a decision record; add one in docs/adr with \"- Allows: third-party/shim/shimx64.efi\"")
}

func TestFindingsAreSorted(t *testing.T) {
	got := check(t, map[string]string{
		"b.md":   "xxxx" + emDash + "\n" + emDash + emDash,
		"a.md":   "x" + emDash,
		"c/x.rs": "// TODO first\n" + emDash,
	})
	want := []string{"a.md:1:2", "b.md:1:5", "b.md:2:1", "b.md:2:4", "c/x.rs:1:4", "c/x.rs:2:1"}
	for i, w := range want {
		if !strings.HasPrefix(got[i], w+":") {
			t.Fatalf("finding %d is %q, want it at %s", i, got[i], w)
		}
	}
}

func TestFindingString(t *testing.T) {
	f := Finding{Path: "a/b.rs", Line: 3, Col: 9, Rule: "stub", Msg: "no"}
	if f.String() != "a/b.rs:3:9: stub: no" {
		t.Fatalf("got %q", f.String())
	}
}

func TestPosition(t *testing.T) {
	data := []byte("ab\ncd\n\nxyz")
	tests := []struct{ off, line, col int }{
		{0, 1, 1}, {1, 1, 2}, {2, 1, 3}, {3, 2, 1}, {4, 2, 2}, {6, 3, 1}, {7, 4, 1}, {9, 4, 3},
	}
	for _, tt := range tests {
		if line, col := position(data, tt.off); line != tt.line || col != tt.col {
			t.Errorf("position(%d) = %d:%d, want %d:%d", tt.off, line, col, tt.line, tt.col)
		}
	}
}

func TestWordIndex(t *testing.T) {
	tests := []struct {
		s, word string
		want    int
	}{
		{"TODO", "TODO", 0},
		{"// TODO: x", "TODO", 3},
		{"TODOS", "TODO", -1},
		{"MYTODO", "TODO", -1},
		{"TODO_LATER", "TODO", -1},
		{"TODO1", "TODO", -1},
		{"xTODO TODO", "TODO", 6},
		{"(TODO)", "TODO", 1},
		{"todo", "TODO", -1},
		{"", "TODO", -1},
	}
	for _, tt := range tests {
		at, ok := wordIndex(tt.s, tt.word)
		if !ok {
			at = -1
		}
		if at != tt.want || ok != (tt.want >= 0) {
			t.Errorf("wordIndex(%q, %q) = %d, %v; want %d", tt.s, tt.word, at, ok, tt.want)
		}
	}
}

func TestIsRawModule(t *testing.T) {
	tests := map[string]bool{
		"k/src/raw.rs":        true,
		"k/src/raw/io.rs":     true,
		"k/src/a/raw/b/c.rs":  true,
		"k/src/rawish.rs":     false,
		"k/src/my_raw.rs":     false,
		"k/src/rawx/a.rs":     false,
		"k/src/lib.rs":        false,
		"raw/lib.rs":          true,
		"k/src/raw.rs.backup": false,
	}
	for p, want := range tests {
		if got := isRawModule(p); got != want {
			t.Errorf("isRawModule(%q) = %v, want %v", p, got, want)
		}
	}
}

func TestRustRulesStayInRust(t *testing.T) {
	expect(t, check(t, map[string]string{
		"k/src/entry.s": "unsafe:\n    call todo\n    ret\n",
		"k/link.ld":     "unsafe = 1;\n",
		"t/a.go":        "package a\n\nfunc f() { todo() }\n",
	}))
}

func TestEveryBinaryExtensionIsSkipped(t *testing.T) {
	files := map[string]string{}
	for _, ext := range []string{".png", ".jpg", ".efi", ".ttf", ".otf", ".fd", ".img", ".iso", ".bin", ".der", ".PNG", ".Img"} {
		files["assets/x"+ext] = emDash
	}
	expect(t, check(t, files))
	expect(t, check(t, map[string]string{"assets/x.pngx": emDash}), "assets/x.pngx:1:1: dash: em dash"+dashMsg)
}

func TestWordEdges(t *testing.T) {
	for s, want := range map[string]int{
		"@TODO": 1, "[TODO]": 1, "`TODO": 1, "{TODO}": 1, "TODO~": 0, "/TODO": 1, "TODO:": 0,
		"0TODO": -1, "9TODO": -1, "TODO0": -1, "TODO9": -1, "ATODO": -1, "ZTODO": -1, "aTODO": -1, "zTODO": -1, "_TODO": -1,
	} {
		at, ok := wordIndex(s, "TODO")
		if !ok {
			at = -1
		}
		if at != want {
			t.Errorf("wordIndex(%q) = %d, want %d", s, at, want)
		}
	}
}

func TestRecordsWithoutStatusAllowNothing(t *testing.T) {
	got := check(t, map[string]string{
		"Cargo.lock":            lockWithRegistry,
		"docs/adr/0007-spin.md": "# 0007. Use spin\n\n- Allows: spin, x86_64\n",
	})
	if len(got) != 2 {
		t.Fatalf("a record with no status line is not accepted: %v", got)
	}
}

func TestBareRequireLine(t *testing.T) {
	expect(t, check(t, map[string]string{"go.mod": "module x\n\nrequire\n"}))
}

func TestLinesAfterARequireBlockAreNotModules(t *testing.T) {
	got := check(t, map[string]string{
		"go.mod":             "module x\n\nrequire (\n\tgithub.com/a/b v1.0.0\n)\n\ntoolchain go1.27.2\n",
		"docs/adr/0003-b.md": "- Status: accepted\n- Allows: github.com/a/b\n",
	})
	expect(t, got)
}
